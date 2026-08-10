//! CSS whitespace processing and shaped-run range assembly.

use core::ops::Range;

use stylo::computed_values::white_space_collapse;

use crate::style::{TextRun, TextRunStyle};

/// One already-measured atomic box participating in an inline paragraph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtomicInlineBox {
    /// Host-stable identifier returned with the positioned box.
    pub id: u64,
    /// Used inline-axis size of the box.
    pub width: f32,
    /// Used block-axis size of the box.
    pub height: f32,
    /// First-baseline offset from the top edge. A synthesized baseline uses
    /// the bottom edge (`baseline == height`).
    pub baseline: f32,
}

impl AtomicInlineBox {
    #[must_use]
    pub const fn new(id: u64, width: f32, height: f32) -> Self {
        Self {
            id,
            width,
            height,
            baseline: height,
        }
    }

    #[must_use]
    pub const fn with_baseline(mut self, baseline: f32) -> Self {
        self.baseline = baseline;
        self
    }
}

/// One source-order item in an inline paragraph.
#[derive(Debug)]
pub enum InlineItem<'a, R: TextRunStyle> {
    Text(TextRun<'a, R>),
    Atomic(AtomicInlineBox),
}

impl<R: TextRunStyle> Clone for InlineItem<'_, R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<R: TextRunStyle> Copy for InlineItem<'_, R> {}

impl<'a, R: TextRunStyle> From<TextRun<'a, R>> for InlineItem<'a, R> {
    fn from(run: TextRun<'a, R>) -> Self {
        Self::Text(run)
    }
}

impl<R: TextRunStyle> From<AtomicInlineBox> for InlineItem<'_, R> {
    fn from(inline_box: AtomicInlineBox) -> Self {
        Self::Atomic(inline_box)
    }
}

/// Whitespace-normalized paragraph content and its non-overlapping run ranges.
pub(super) struct ShapingContent<'a, R: TextRunStyle> {
    pub(super) text: String,
    pub(super) ranges: Vec<StyledRange<'a, R>>,
    pub(super) boxes: Vec<ShapingInlineBox>,
}

/// One contiguous range in normalized UTF-8 text carrying a host run style.
pub(super) struct StyledRange<'a, R: TextRunStyle> {
    pub(super) bytes: Range<usize>,
    pub(super) style: &'a R,
}

/// One atomic box and its insertion point in normalized UTF-8 text.
pub(super) struct ShapingInlineBox {
    pub(super) inline_box: AtomicInlineBox,
    pub(super) index: usize,
}

pub(super) fn normalize_runs<'a, R, Runs>(
    runs: Runs,
    collapse: white_space_collapse::T,
) -> ShapingContent<'a, R>
where
    R: TextRunStyle + 'a,
    Runs: Iterator<Item = TextRun<'a, R>> + Clone,
{
    normalize_items(runs.map(InlineItem::Text), collapse)
}

pub(super) fn normalize_items<'a, R, Items>(
    items: Items,
    collapse: white_space_collapse::T,
) -> ShapingContent<'a, R>
where
    R: TextRunStyle + 'a,
    Items: Iterator<Item = InlineItem<'a, R>> + Clone,
{
    let preserves_spaces = matches!(
        collapse,
        white_space_collapse::T::Preserve | white_space_collapse::T::BreakSpaces
    );
    let (text_capacity, range_capacity, box_capacity) = if preserves_spaces {
        items.clone().fold(
            (0, 0, 0),
            |(text_bytes, run_count, box_count), item| match item {
                InlineItem::Text(run) => (text_bytes + run.text.len(), run_count + 1, box_count),
                InlineItem::Atomic(_) => (text_bytes, run_count, box_count + 1),
            },
        )
    } else {
        let (lower, upper) = items.size_hint();
        let item_capacity = upper.unwrap_or(lower);
        (0, item_capacity, item_capacity)
    };
    let mut content = ShapingContent {
        text: String::with_capacity(text_capacity),
        ranges: Vec::with_capacity(range_capacity),
        boxes: Vec::with_capacity(box_capacity),
    };

    if preserves_spaces {
        for item in items {
            match item {
                InlineItem::Text(run) => content.push_str(run.text, run.style),
                InlineItem::Atomic(inline_box) => content.push_inline_box(inline_box),
            }
        }
        return content;
    }
    let force_preserved_breaks = collapse == white_space_collapse::T::PreserveBreaks;

    let mut pending_whitespace = None;
    let mut after_preserved_break = false;

    for item in items {
        match item {
            InlineItem::Text(run) => {
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
                            flush_pending_whitespace(
                                &mut content,
                                &mut pending_whitespace,
                                character,
                            );
                            content.push(character, run.style);
                            after_preserved_break = false;
                        }
                    }
                }
            }
            InlineItem::Atomic(inline_box) => {
                if let Some(whitespace) = pending_whitespace.take() {
                    content.push(' ', whitespace.style());
                }
                content.push_inline_box(inline_box);
                after_preserved_break = false;
            }
        }
    }

    if !content.is_empty()
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
        && should_remove_segment_break(content.previous_text_character(), next);
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
    fn is_empty(&self) -> bool {
        self.text.is_empty() && self.boxes.is_empty()
    }

    fn previous_text_character(&self) -> Option<char> {
        if self
            .boxes
            .last()
            .is_some_and(|inline_box| inline_box.index == self.text.len())
        {
            None
        } else {
            self.text.chars().next_back()
        }
    }

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

    fn push_inline_box(&mut self, inline_box: AtomicInlineBox) {
        self.boxes.push(ShapingInlineBox {
            inline_box,
            index: self.text.len(),
        });
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
            boxes: Vec::new(),
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
            boxes: Vec::new(),
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

    #[test]
    fn inline_boxes_keep_source_order_at_normalized_text_offsets() {
        let style = RunStyle(1);
        let items = [
            InlineItem::Text(run(&style, "a  ", false)),
            InlineItem::Atomic(AtomicInlineBox::new(11, 20.0, 10.0)),
            InlineItem::Atomic(AtomicInlineBox::new(12, 30.0, 12.0)),
            InlineItem::Text(run(&style, "  b", false)),
        ];

        let content = normalize_items(items.into_iter(), white_space_collapse::T::Collapse);

        assert_eq!(content.text, "a  b");
        assert_eq!(content.boxes.len(), 2);
        assert_eq!(content.boxes[0].inline_box.id, 11);
        assert_eq!(content.boxes[0].index, 2);
        assert_eq!(content.boxes[1].inline_box.id, 12);
        assert_eq!(content.boxes[1].index, 2);
    }

    #[test]
    fn atomic_boundaries_stop_segment_break_character_elision() {
        let style = RunStyle(1);
        let items = [
            InlineItem::Text(run(&style, "你", false)),
            InlineItem::Atomic(AtomicInlineBox::new(1, 10.0, 10.0)),
            InlineItem::Text(run(&style, "\n好", false)),
        ];

        let content = normalize_items(items.into_iter(), white_space_collapse::T::Collapse);

        assert_eq!(content.text, "你 好");
        assert_eq!(content.boxes[0].index, "你".len());
    }

    #[test]
    fn pure_atomic_content_survives_normalization() {
        let style = RunStyle(1);
        let items = [InlineItem::<RunStyle>::Atomic(AtomicInlineBox::new(
            7, 40.0, 16.0,
        ))];

        let content = normalize_items(items.into_iter(), white_space_collapse::T::Collapse);

        assert!(content.text.is_empty());
        assert!(content.ranges.is_empty());
        assert_eq!(content.boxes.len(), 1);
        assert_eq!(content.boxes[0].index, 0);

        let mixed = normalize_items(
            [
                InlineItem::Atomic(AtomicInlineBox::new(7, 40.0, 16.0)),
                InlineItem::Text(run(&style, " ", false)),
            ]
            .into_iter(),
            white_space_collapse::T::Collapse,
        );
        assert_eq!(mixed.text, " ");
    }
}
