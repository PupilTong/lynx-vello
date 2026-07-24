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

pub(super) fn normalize_runs<'a, R, Runs>(
    runs: Runs,
    collapse: white_space_collapse::T,
) -> ShapingContent<'a, R>
where
    R: TextRunStyle + 'a,
    Runs: Iterator<Item = TextRun<'a, R>> + Clone,
{
    let preserves_spaces = matches!(
        collapse,
        white_space_collapse::T::Preserve | white_space_collapse::T::BreakSpaces
    );
    let (text_capacity, range_capacity) = if preserves_spaces {
        runs.clone().fold((0, 0), |(text_bytes, run_count), run| {
            (text_bytes + run.text.len(), run_count + 1)
        })
    } else {
        let (lower, upper) = runs.size_hint();
        (0, upper.unwrap_or(lower))
    };
    let mut content = ShapingContent {
        text: String::with_capacity(text_capacity),
        ranges: Vec::with_capacity(range_capacity),
    };

    if preserves_spaces {
        for run in runs {
            content.push_str(run.text, run.style);
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
        self.record_append(start, style);
    }

    fn push_str(&mut self, text: &str, style: &'a R) {
        let start = self.text.len();
        self.text.push_str(text);
        self.record_append(start, style);
    }

    fn record_append(&mut self, start: usize, style: &'a R) {
        let end = self.text.len();
        if start == end {
            return;
        }
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
    struct RunStyle(u8);

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

    fn run<'a>(
        style: &'a RunStyle,
        text: &'a str,
        preserve_newlines: bool,
    ) -> TextRun<'a, RunStyle> {
        TextRun {
            text,
            style,
            preserve_newlines,
        }
    }

    fn normalize_one<'a>(
        style: &'a RunStyle,
        text: &'a str,
        preserve_newlines: bool,
        collapse: white_space_collapse::T,
    ) -> ShapingContent<'a, RunStyle> {
        normalize_runs([run(style, text, preserve_newlines)].into_iter(), collapse)
    }

    #[test]
    fn collapses_css_whitespace_across_run_boundaries() {
        let first = RunStyle(1);
        let second = RunStyle(2);
        let runs = [
            run(&first, "a \t\r\n", false),
            run(&second, "  b\u{a0}c", false),
        ];

        let content = normalize_runs(runs.into_iter(), white_space_collapse::T::Collapse);

        assert_eq!(content.text, "a b\u{a0}c");
        assert_eq!(content.ranges.len(), 2);
        assert_eq!(content.ranges[0].bytes, 0..2);
        assert_eq!(content.ranges[1].bytes, 2..6);
    }

    #[test]
    fn raw_text_preserves_breaks_and_removes_adjacent_spaces() {
        let style = RunStyle(1);
        let content = normalize_one(
            &style,
            "one \r\n \t two\x0Cthree",
            true,
            white_space_collapse::T::Collapse,
        );

        assert_eq!(content.text, "one\ntwo\x0Cthree");
        assert_eq!(content.ranges[0].bytes, 0..13);
    }

    #[test]
    fn normalizes_cross_run_crlf_and_chromium_segment_breaks() {
        let first = RunStyle(1);
        let second = RunStyle(2);
        let raw = normalize_runs(
            [run(&first, "a\r", true), run(&second, "\nb", true)].into_iter(),
            white_space_collapse::T::Collapse,
        );
        assert_eq!(raw.text, "a\nb");

        for (source, preserve_newlines, expected) in [
            ("a\rb\x0Cc", true, "a b\x0Cc"),
            ("你\n好", false, "你好"),
            ("안\n녕", false, "안 녕"),
            ("a\n\u{200B}b", false, "a\u{200B}b"),
        ] {
            assert_eq!(
                normalize_one(
                    &first,
                    source,
                    preserve_newlines,
                    white_space_collapse::T::Collapse
                )
                .text,
                expected,
                "for source {source:?}"
            );
        }
    }

    #[test]
    fn preserve_modes_pass_text_through_unmodified() {
        let style = RunStyle(1);
        for collapse in [
            white_space_collapse::T::Preserve,
            white_space_collapse::T::BreakSpaces,
        ] {
            let content = normalize_one(&style, "a  \t b\n c", false, collapse);
            assert_eq!(content.text, "a  \t b\n c");
            assert_eq!(content.ranges.len(), 1);
            assert_eq!(content.ranges[0].bytes, 0..9);
        }
    }

    #[test]
    fn preserve_modes_merge_only_adjacent_pointer_identical_ranges() {
        let first = RunStyle(1);
        let second = RunStyle(2);
        for collapse in [
            white_space_collapse::T::Preserve,
            white_space_collapse::T::BreakSpaces,
        ] {
            let content = normalize_runs(
                [
                    run(&first, "", false),
                    run(&first, "é", false),
                    run(&second, "", false),
                    run(&first, "你\n", false),
                    run(&second, " \t", false),
                    run(&second, "🙂", false),
                    run(&first, "", false),
                    run(&first, "x", false),
                ]
                .into_iter(),
                collapse,
            );

            assert_eq!(content.text, "é你\n \t🙂x");
            assert_eq!(
                content
                    .ranges
                    .iter()
                    .map(|range| (range.bytes.clone(), range.style.0))
                    .collect::<Vec<_>>(),
                [(0..6, 1), (6..12, 2), (12..13, 1)]
            );

            let empty = normalize_runs(
                [run(&first, "", false), run(&second, "", false)].into_iter(),
                collapse,
            );
            assert!(empty.text.is_empty());
            assert!(empty.ranges.is_empty());
        }
    }

    #[test]
    fn trailing_collapsible_whitespace_keeps_one_space() {
        let style = RunStyle(1);
        let content = normalize_one(&style, "a b \t ", false, white_space_collapse::T::Collapse);
        assert_eq!(content.text, "a b ");
        assert_eq!(content.ranges[0].bytes, 0..4);
    }

    #[test]
    fn segment_break_removal_covers_supplementary_east_asian_blocks() {
        let style = RunStyle(1);
        for (source, expected) in [
            ("\u{F900}\n\u{F900}", "\u{F900}\u{F900}"),
            ("\u{FE10}\n\u{FE10}", "\u{FE10}\u{FE10}"),
            ("\u{FE30}\n\u{FE30}", "\u{FE30}\u{FE30}"),
            ("\u{FF01}\n\u{FF01}", "\u{FF01}\u{FF01}"),
            ("\u{FFE0}\n\u{FFE0}", "\u{FFE0}\u{FFE0}"),
            ("\u{17000}\n\u{17000}", "\u{17000}\u{17000}"),
            ("\u{1AFF0}\n\u{1AFF0}", "\u{1AFF0}\u{1AFF0}"),
            ("\u{1F200}\n\u{1F200}", "\u{1F200}\u{1F200}"),
            ("\u{20000}\n\u{20000}", "\u{20000}\u{20000}"),
        ] {
            let content = normalize_one(&style, source, false, white_space_collapse::T::Collapse);
            assert_eq!(content.text, expected, "for source {source:?}");
        }
    }

    #[test]
    fn remove_trailing_space_shrinks_and_drops_emptied_ranges() {
        let style = RunStyle(1);
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

        let mut only_space = ShapingContent::<'_, RunStyle> {
            text: String::new(),
            ranges: Vec::new(),
        };
        only_space.push(' ', &style);
        only_space.remove_trailing_space();
        assert!(only_space.text.is_empty());
        assert!(only_space.ranges.is_empty());

        content.remove_trailing_space();
        assert_eq!(content.text, "a");
    }

    #[test]
    fn fully_collapsible_and_empty_inputs_produce_no_shaping_ranges() {
        let first = RunStyle(1);
        let second = RunStyle(2);
        let whitespace = normalize_runs(
            [run(&first, " \t\r", false), run(&second, "\n ", false)].into_iter(),
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
