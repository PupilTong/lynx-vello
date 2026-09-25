// Cards read against their own source, whose rendering this engine gets right.
// The golden beside this file is that rendering, so a failure is a change to
// judge — never a difference from another engine.
//
// Each entry names what the card tests, because that is what the golden is
// evidence of; a rendering nobody can restate is not a verified one.

verified! {
    // `var()` chained through a second custom property resolves to pink.
    basic_css_var => "basic-css-var",
    // `background-image: url(…)` from CSS, framed by `background-size: contain`.
    basic_css_asset_in_css => "basic-css-asset-in-css",
    // `.a.b` (0,2,0) beats `.wrapper view` (0,1,1) on the first box alone.
    basic_css_compound_selector => "basic-css-compound-selector",
    // The later class wins, and the gradient fills the letterforms themselves.
    basic_element_text_color => "basic-element-text-color",
    // `display: none` on a `<text>` still paints: the 2026-09-14 ruling.
    basic_element_text_display_none => "basic-element-text-display-none",
    // Class against inline text styling across four runs of a flex row.
    basic_element_text_dynamic_text_style_update => "basic-element-text-dynamic-text-style-update",
    // An `@font-face` family reaches shaping.
    basic_element_text_extra_font_family => "basic-element-text-extra-font-family",
    // `text-maxline` 1 / 2 / unset clamps the wrapped paragraph, and the
    // marker stays gated on `text-overflow` (2026-09-14 ruling).
    basic_element_text_maxline => "basic-element-text-maxline",
    // `text-maxlength` cuts by character, including CJK and a mid-word 200.
    basic_element_text_maxlength => "basic-element-text-maxlength",
    // A 22x22 `<image>` as an inline atom, its `margin-left` on the line.
    basic_element_text_nest_image => "basic-element-text-nest-image",
    // A `<view>` inside a `<text>` as an atomic inline box.
    basic_element_text_nest_view => "basic-element-text-nest-view",
    // `tail-color-convert` follows native: the marker wears the cut run.
    basic_element_text_tail_color_convert => "basic-element-text-tail-color-convert",
    // A gradient paints through the paragraph's own runs.
    basic_element_text_text_with_linear_gradient => "basic-element-text-text-with-linear-gradient",
    // A literal newline in a `raw-text` breaks the line.
    basic_element_text_with_new_line => "basic-element-text-with-new-line",
    // Digits, Latin and CJK break where `word-break` says they do.
    basic_element_text_word_break => "basic-element-text-word-break",
}
