//! SVG documents used as images: parsed by the host, drawn as an isolated
//! SVG document cascaded by stylo.
//!
//! A host turns a document's bytes into a [`VectorDocument`] with
//! `ImageEvent::parse_document`, on whatever thread it decodes bitmaps on:
//! the parsed [`XmlMarkup`] and the natural size the root's attributes give,
//! nothing that needs a document or fonts, so it is `Send + Sync`. On the
//! document thread, applying it ([`vector_image`]) builds an
//! [`IsolatedSvg`]: a `Document<()>` of its own whose document element holds
//! the one `svg` element (`Document::append_svg_markup`), with the SVG
//! user-agent sheet and [`IMAGE_SHEET`](super::ua) mounted, its styles
//! flushed. Its subtree is recorded with [`RootGroup::Recorded`] (an image's
//! root has no box to carry its own `opacity`, `clip-path`, mask and
//! `transform`) and its text shaped with the host document's `TextContext`,
//! so SVG glyphs come from the page's fonts and `@font-face` registrations.
//! The recording is then drawn as an inline root's is: planned into paint
//! items and group layers (`Document::plan_svg_image`) and painted by the
//! paint walk, with nothing culled, into the image's one display list. The
//! root is not a CSS box: the document is never laid out or rendered. The
//! isolated document is dropped once the image is drawn.
//!
//! CSS inside the document applies: its `style` elements are author sheets
//! of the isolated document and its `style` attributes are inline styles,
//! both through stylo.
//!
//! The parse reads an element in no namespace as an SVG element, so markup
//! written without `xmlns` (`<svg viewBox="…">`) draws, as it did under the
//! engine's earlier converter; a browser refuses such a document as an
//! image. This is SVG-image policy, passed to the tree's XML parser
//! ([`XmlMarkup::parse`]), which has no default of its own. The root must
//! then be the SVG `svg` element ([`SvgError::NotSvg`]).

use std::cell::Cell;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use seldon::text::TextContext;

use super::SVG_TAG;
use super::attributes::{absolute_length, vector_sizes, view_box};
use super::record::{RootGroup, record};
#[cfg(test)]
use super::recording::Recording;
use super::text::shape;
use crate::paint::display_list::DisplayList;
use crate::paint::walker::{Scratch, walk_uncultured};
use crate::render::image::{NoImages, VectorImage};
use crate::style::device::Device;
use crate::tree::document::{Document, NodeId};
use crate::tree::xml::{XmlError, XmlItem, XmlMarkup, XmlNamespace};

#[derive(Clone, Debug)]
pub struct VectorDocument {
    markup: XmlMarkup,
    pub(crate) natural: (u32, u32),
}

const _: () = {
    const fn send_sync<T: Send + Sync>() {}
    send_sync::<VectorDocument>();
};

impl VectorDocument {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, SvgError> {
        let markup =
            XmlMarkup::parse(bytes, Some(XmlNamespace::Svg)).map_err(|error| match error {
                XmlError::ForeignRoot => SvgError::NotSvg,
                error => SvgError::Xml(error),
            })?;
        let XmlItem::Element {
            namespace: XmlNamespace::Svg,
            local_name: SVG_TAG,
            attributes,
        } = markup.item(0)
        else {
            return Err(SvgError::NotSvg);
        };
        let mut width = None;
        let mut height = None;
        let mut root_view_box = None;
        for &attribute in attributes {
            match markup.attribute(attribute) {
                ("width", value) => width = absolute_length(value),
                ("height", value) => height = absolute_length(value),
                ("viewBox", value) => root_view_box = view_box(value).map(|v| (v.w, v.h)),
                _ => {}
            }
        }
        let (natural, _viewport) = vector_sizes(width, height, root_view_box);
        Ok(Self { markup, natural })
    }
}

#[derive(Debug)]
pub(crate) enum SvgError {
    Xml(XmlError),
    NotSvg,
}

impl std::fmt::Display for SvgError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Xml(error) => error.fmt(formatter),
            Self::NotSvg => formatter.write_str("the root element is not <svg>"),
        }
    }
}

impl std::error::Error for SvgError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Xml(error) => Some(error),
            Self::NotSvg => None,
        }
    }
}

pub(crate) struct IsolatedSvg {
    document: Document<()>,
    root: NodeId,
}

impl IsolatedSvg {
    pub(crate) fn build(parsed: &VectorDocument, now: f64) -> Self {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a whole-px natural size, far below f32's integer range"
        )]
        let (width, height) = (parsed.natural.0 as f32, parsed.natural.1 as f32);
        let mut document = Document::new(Device::new(width, height, 1.0), "svg-image", ());
        document.sync_animation_clock(now);
        document.add_svg_user_agent_stylesheet();
        document.add_svg_image_stylesheet();
        let parent = document.document_element().id();
        let root = document.append_svg_markup(parent, &parsed.markup, || ());
        document.flush_styles_with_damage_sink(&mut |_, _| {});
        Self { document, root }
    }

    #[cfg(test)]
    pub(crate) fn record(&self) -> Recording {
        record(&self.document, self.root, RootGroup::Recorded)
    }

    pub(crate) fn image(&mut self, context: &mut Option<Box<TextContext>>) -> VectorImage {
        let document = &mut self.document;
        let mut recording = record(document, self.root, RootGroup::Recorded);
        shape(&mut recording, context);
        let (viewport, aspect) = (recording.viewport, recording.aspect);
        document.keep_svg_recording(self.root, recording);
        let plan = document.plan_svg_image(self.root);
        let mut scene = DisplayList::new();
        walk_uncultured(
            &mut scene,
            &mut Scratch::default(),
            document,
            &plan,
            &document.images,
            &NoImages,
        );
        VectorImage::new(scene, viewport, aspect)
    }

    pub(crate) fn runs(&self) -> bool {
        self.document.has_active_animations()
    }
}

#[derive(Default)]
pub(crate) struct AnimatedImages {
    entries: FxHashMap<Arc<str>, AnimatedImage>,
}

struct AnimatedImage {
    isolated: IsolatedSvg,
    drawn: Cell<bool>,
}

impl AnimatedImages {
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn remove(&mut self, source: &str) {
        if self.entries.remove(source).is_some() && self.entries.is_empty() {
            *self = Self::default();
        }
    }
}

impl<T> Document<T> {
    fn image_in_use(&self, source: &str, animated: &AnimatedImage) -> bool {
        animated.drawn.get() || self.images.presenters(source) > 0
    }

    pub(crate) fn animated_images_run(&self) -> bool {
        !self.animated_images.is_empty()
            && self
                .animated_images
                .entries
                .iter()
                .any(|(source, animated)| self.image_in_use(source, animated))
    }

    pub(crate) fn note_drawn_images(&self, draws: &[crate::paint::compose::VectorDraw]) -> bool {
        if self.animated_images.is_empty() {
            return false;
        }
        for (source, animated) in &self.animated_images.entries {
            let key = self.images.vector_key(source);
            animated
                .drawn
                .set(key.is_some_and(|key| draws.iter().any(|draw| draw.key == key)));
        }
        self.animated_images_run()
    }

    pub(crate) fn apply_image_event(
        &mut self,
        event: &crate::render::image::ImageEvent,
    ) -> Option<crate::render::image::ImageStatus> {
        let now = self.animations().now();
        let mut running = None;
        let applied = {
            let (images, context) = self.image_apply_parts();
            images.apply(event, |parsed| {
                let mut isolated = IsolatedSvg::build(parsed, now);
                let image = isolated.image(context);
                if isolated.runs() {
                    running = Some(isolated);
                }
                image
            })
        };
        if let (Some(isolated), Some(_), Some(source)) = (running, applied, event.source()) {
            self.animated_images.entries.insert(
                Arc::clone(source),
                AnimatedImage {
                    isolated,
                    drawn: Cell::new(false),
                },
            );
        }
        applied
    }

    pub(crate) fn forget_animated_image(&mut self, source: &str) {
        if !self.animated_images.is_empty() && !self.images.knows(source) {
            self.animated_images.remove(source);
        }
    }
}

impl<T: Sync> Document<T> {
    pub(crate) fn advance_animated_images(
        &mut self,
        now: f64,
        tick: &mut crate::style::animation::AnimationTick,
    ) {
        let mut table = std::mem::take(&mut self.animated_images);
        table.entries.retain(|source, animated| {
            if !self.image_in_use(source, animated) {
                return true;
            }
            let moved = animated.isolated.document.advance_animations(now);
            if moved.restyled > 0 {
                let image = animated
                    .isolated
                    .image(&mut self.layout_state_mut().text_context);
                self.images.replace_vector(source, image);
                self.note_visual_mutation();
                tick.restyled += moved.restyled;
            }
            let runs = animated.isolated.runs();
            tick.needs_next_frame |= runs;
            runs
        });
        if table.entries.is_empty() {
            table = AnimatedImages::default();
        }
        self.animated_images = table;
    }
}

#[cfg(test)]
pub(crate) fn vector_image(
    parsed: &VectorDocument,
    context: &mut Option<Box<TextContext>>,
) -> VectorImage {
    IsolatedSvg::build(parsed, 0.0).image(context)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;

    use crate::render::image::{DocumentKind, ImageEvent};
    use crate::render::picture::PictureKey;
    use crate::test_common::Doc;
    use crate::tree::document::NodeId;

    const SVG_OPEN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">"#;

    fn presented(source: &str, svg: &str) -> (Doc, NodeId) {
        let mut doc = Doc::with_css("image { display: flex; width: 40px; height: 40px; }");
        let node = doc.el(doc.root, "image");
        let event =
            ImageEvent::parse_document(Arc::from(source), svg.as_bytes(), DocumentKind::Svg);
        doc.dom.apply_image_events(&[event]);
        let natural = doc.dom.request_image(source).natural_size();
        doc.dom
            .set_picture(node, Some((PictureKey::Url(Arc::from(source)), natural)));
        doc.dom.render();
        (doc, node)
    }

    fn key(doc: &Doc, source: &str) -> u64 {
        doc.dom
            .images
            .vector_key(source)
            .expect("a settled vector image")
    }

    const FADING: &str = r#"<style>
          @keyframes paint { from { fill: #ff0000; } to { fill: #0000ff; } }
          rect { animation: paint 1s linear; }
        </style><rect width="20" height="20"/></svg>"#;

    #[test]
    fn an_image_that_cannot_change_keeps_no_document() {
        for body in [
            r#"<rect width="20" height="20" fill="red"/></svg>"#,
            r#"<style>@keyframes unused { to { fill: blue; } }</style>
               <rect width="20" height="20" fill="red"/></svg>"#,
            r#"<rect width="20" height="20" fill="red">
                 <animate attributeName="fill" to="blue" dur="1s"/></rect></svg>"#,
        ] {
            let (doc, _) = presented("app:///still.svg", &format!("{SVG_OPEN}{body}"));
            assert!(doc.dom.animated_images.is_empty(), "{body}");
            assert!(!doc.dom.has_active_animations(), "{body}");
        }
    }

    #[test]
    fn an_animated_image_ticks_with_the_host_and_ends() {
        let source = "app:///fading.svg";
        let (mut doc, _) = presented(source, &format!("{SVG_OPEN}{FADING}"));
        assert!(!doc.dom.animated_images.is_empty(), "kept while it runs");
        assert!(doc.dom.has_active_animations());
        assert!(doc.dom.commit().needs_main_ticks());
        doc.dom.advance_animations(0.0);
        let mut previous = key(&doc, source);
        for now in [0.25, 0.5] {
            let tick = doc.dom.advance_animations(now);
            assert!(
                tick.needs_next_frame && tick.restyled > 0,
                "{now}: {tick:?}"
            );
            assert!(doc.dom.needs_render(), "{now}: the host owes a repaint");
            let next = key(&doc, source);
            assert_ne!(next, previous, "{now}");
            previous = next;
            assert!(doc.dom.commit().needs_main_ticks());
        }
        let tick = doc.dom.advance_animations(2.0);
        assert!(!tick.needs_next_frame, "{tick:?}");
        assert!(doc.dom.animated_images.is_empty(), "dropped at its end");
        let ended = key(&doc, source);
        assert!(!doc.dom.commit().needs_main_ticks());
        doc.dom.advance_animations(3.0);
        assert_eq!(key(&doc, source), ended, "the last image stays");
    }

    #[test]
    fn an_animated_image_nothing_uses_is_not_ticked() {
        let source = "app:///fading.svg";
        let (mut doc, node) = presented(source, &format!("{SVG_OPEN}{FADING}"));
        doc.dom.advance_animations(0.0);
        doc.dom.set_picture(node, None);
        assert!(!doc.dom.commit().needs_main_ticks(), "nothing uses it");
        assert!(!doc.dom.has_active_animations());
        let unused = key(&doc, source);
        let tick = doc.dom.advance_animations(0.5);
        assert!(!tick.needs_next_frame);
        assert_eq!(key(&doc, source), unused, "not ticked");
        let natural = doc.dom.request_image(source).natural_size();
        doc.dom
            .set_picture(node, Some((PictureKey::Url(Arc::from(source)), natural)));
        doc.dom.commit();
        assert!(doc.dom.has_active_animations());
        doc.dom.advance_animations(0.6);
        assert_ne!(key(&doc, source), unused, "ticked again once used");
    }

    #[test]
    fn a_synthetic_source_takes_its_document_with_it() {
        let mut doc = Doc::with_css("image { display: flex; width: 40px; height: 40px; }");
        let node = doc.el(doc.root, "image");
        let markup = format!("{SVG_OPEN}{FADING}");
        let source = doc
            .dom
            .insert_image_document(markup.as_bytes(), DocumentKind::Svg);
        doc.dom
            .set_picture(node, Some((PictureKey::Url(Arc::clone(&source)), None)));
        let events: Vec<_> = doc
            .dom
            .take_document_requests()
            .into_iter()
            .map(|(source, bytes, kind)| ImageEvent::parse_document(source, &bytes, kind))
            .collect();
        doc.dom.apply_image_events(&events);
        assert!(!doc.dom.animated_images.is_empty());
        doc.dom.set_picture(node, None);
        assert!(!doc.dom.images.knows(&source));
        assert!(doc.dom.animated_images.is_empty());
    }
}
