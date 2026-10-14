//! The `<markdown>` / `<x-markdown>` tags, web-core's `x-markdown`
//! (`lynx-stack/packages/web-platform/web-elements/src/elements/XMarkdown/`):
//! an element that renders its `content` attribute as Markdown in its shadow
//! tree.
//!
//! The component that builds and updates that shadow tree is written here
//! next. What it reads is in the two modules below, which hold no DOM code:
//! [`parse`] turns `content` (and `content-range`) into an owned tree of
//! blocks and inlines, carrying the source and rendered-text facts the UI
//! methods answer with, and [`style`] turns `markdown-style` into the text
//! of the shadow tree's stylesheet.

// Until the component reads them, nothing outside the tests does.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the `<markdown>` component that reads these modules is not written yet"
    )
)]

pub(super) mod parse;
pub(super) mod style;
