//! An `svg` element in a document draws its own subtree.
//!
//! An *outermost* SVG `svg` element, one whose flat-tree parent is not an
//! SVG element, is a replaced box (`super::root`) whose picture is its
//! subtree. The document keeps one [`Recording`] per such root in a side
//! table, [`SvgRoots`], empty and unallocated while the document has none.
//! The visual plan turns each recorded path and text item into a paint item
//! of the ordinary paint order, after the root's own box item, and each
//! layer push into a group scope around the items it holds
//! (`visual::svg`); the paint walk draws them into the frame's display list
//! like any other item (`paint::svg`). An inline root makes no vector
//! image, no vector draw and no texture.
//!
//! Where a recording lands in the root's box is
//! [`Document::svg_root_placement`]: the map from its viewport units onto
//! the root's border box, the one a vector image of the same natural size
//! gets (the destination rectangle `object-fit` and `object-position` give
//! in the content box, `paint::replaced_destination`, then the root's
//! `preserveAspectRatio`, `paint::compose::aspect_transform`), and what its
//! drawing is clipped to, the content box's shape cut to that rectangle
//! (`paint::fill_plan`), as the image's draw is.
//!
//! A root is recorded ([`super::record`], with [`RootGroup::Box`]: its own
//! `opacity`, `transform` and clip are its CSS box's) at the start of each
//! frame build, after the style flush and layout and before the visual
//! plan, when it is *dirty*, and its text is shaped with the document's own
//! `TextContext` (`super::text::shape`). It is marked dirty by:
//!
//! - an attribute set or removed on an SVG element (every attribute but `class` and `style`, which
//!   reach the drawing only through the cascade), `id` included;
//! - a child inserted under or removed from an SVG element, an SVG element inserted anywhere, and a
//!   text change under an SVG element;
//! - a style change of an SVG element, as the style flush's and the animation recascade's damage
//!   harvests see it. Every longhand that has no `servo_restyle_damage` of its own reports
//!   `rebuild_box`, which is every SVG paint and geometry property, so `fill` reports relayout
//!   damage and is never empty. The harvest tests the element's namespace bit and nothing more for
//!   an element that is not SVG. An element below its root has no box, so the harvest skips its
//!   layout invalidation ([`Document::note_svg_restyle`]): its relayout damage would otherwise
//!   clear the root's and its ancestors' box caches, or force a layout pass, on every `fill` change
//!   and every animation tick, for geometry that cannot move. The root's own damage takes the
//!   normal path, since its `width`, `height` and `display` are box properties.
//! - the first style of an outermost `svg`, which the style harvest finds with no damage
//!   ([`Document::note_svg_first_style`]): a root under an HTML ancestor that left `display: none`
//!   had no style while hidden, and a `class` or `style` write below it then marked nothing. The
//!   animation recascade styles no element for the first time: `display` does not animate.
//!
//! Each mark walks up the flat tree to the outermost `svg` and files it.
//! A dirty root that is not connected, or has no computed style yet, stays
//! dirty and is recorded at the first build that finds it connected and
//! styled. A filed element that is no longer outermost leaves the table at
//! the first build that finds it dirty: when root `a` is inserted under the
//! `svg` element `b`, the insertion marks `b`, not `a`, and every later
//! mark under `a` walks up to `b`, so `a`'s entry and recording stay until
//! `a` is freed, or is outermost again and marked.
//! Every record replaces the root's recording; an untouched root keeps its
//! recording, and only the plan and the paint walk read it again. A freed
//! root leaves the table in `Document::free_node`.
//!
//! An SVG image's isolated document (`super::image`) files its root's
//! recording in its own table too ([`Document::keep_svg_recording`]),
//! recorded with [`RootGroup::Recorded`] and shaped with the host
//! document's text context, which is where the plan and the paint walk read
//! it; it never builds a frame, so [`Document::record_svg_roots`] never runs
//! for it.

use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use super::record::{RootGroup, record};
use super::recording::Recording;
use super::text::shape;
use crate::kurbo::{Affine, Rect};
use crate::paint::compose::aspect_transform;
use crate::paint::shape::BoxShape;
use crate::paint::{BoxFragment, FillPlan, content_radii, fill_plan, replaced_destination};
use crate::tree::document::{Document, NodeId};
use crate::visual::CornerRadii;
use crate::visual::geometry::resolve_corner_radii;

/// The outermost `svg` elements of a document and what each draws.
#[derive(Debug, Default)]
pub(crate) struct SvgRoots {
    entries: FxHashMap<NodeId, SvgRoot>,
    /// Whether some entry waits for a record.
    dirty: bool,
}

#[derive(Debug, Default)]
struct SvgRoot {
    /// The last record, its text shaped; `None` before the first.
    recording: Option<Recording>,
    dirty: bool,
    /// How many times the root was recorded.
    #[cfg(test)]
    records: u32,
}

impl SvgRoots {
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Takes `id` out of the table, giving the table's storage back once it
    /// is empty.
    fn remove(&mut self, id: NodeId) {
        if self.entries.remove(&id).is_some() && self.entries.is_empty() {
            *self = Self::default();
        }
    }
}

/// Where an `svg` root's recording draws in its border box.
#[derive(Debug)]
pub(crate) struct RootPlacement {
    /// The recording's viewport units to the root's border-box px.
    pub(crate) map: Affine,
    /// The clips the drawing is cut to, outermost first, each a rectangle
    /// with corner radii in border-box px: the content box's shape, the
    /// destination rectangle, or both.
    pub(crate) clips: SmallVec<[(Rect, CornerRadii); 2]>,
}

impl<T> Document<T> {
    /// Marks the outermost `svg` element around `id`, an SVG element or a
    /// node under one, as owing a record, and answers it. Nothing, and
    /// `None`, when `id` is under no `svg` element.
    pub(crate) fn note_svg_change(&mut self, id: NodeId) -> Option<NodeId> {
        let root = self.outermost_svg(id)?;
        let roots = &mut self.svg_roots;
        roots.entries.entry(root).or_default().dirty = true;
        roots.dirty = true;
        self.note_visual_mutation();
        Some(root)
    }

    /// Notes the style change of the SVG element `id` that a damage harvest
    /// found, and answers whether it lies below its outermost `svg`: such
    /// an element generates no box (the root is replaced content,
    /// `super::root`), so its change is its root's new drawing and no
    /// layout work. The root itself is a box, and keeps the harvest's
    /// layout path.
    pub(crate) fn note_svg_restyle(&mut self, id: NodeId) -> bool {
        self.note_svg_change(id).is_some_and(|root| root != id)
    }

    /// Notes the first style of the SVG element `id` that the style
    /// harvest found. A first style carries no damage, so an outermost
    /// `svg` styled again after an HTML ancestor left `display: none` is
    /// marked here: a `class` or `style` write below it while it had no
    /// style marked nothing. An element whose flat-tree parent is an SVG
    /// element is not tested further: that parent's own display change, or
    /// the insertion that brought the element in, marked the root.
    pub(crate) fn note_svg_first_style(&mut self, id: NodeId) {
        let outermost = self.get(id).is_some_and(|node| {
            !node
                .flat_parent()
                .is_some_and(|parent| parent.is_element() && parent.in_svg_namespace())
        });
        if outermost {
            self.note_svg_change(id);
        }
    }

    /// The outermost `svg` element around `id`, an SVG element or a node
    /// under one, along the flat tree.
    fn outermost_svg(&self, id: NodeId) -> Option<NodeId> {
        let mut top = None;
        let mut current = self.get(id);
        while let Some(node) = current {
            if node.is_element() {
                if !node.in_svg_namespace() {
                    break;
                }
                top = Some(node);
            }
            current = node.flat_parent();
        }
        Some(
            top.filter(|node| node.tag_name() == Some(super::SVG_TAG))?
                .id(),
        )
    }

    /// Records every dirty root that is connected and styled, and shapes
    /// its text. One `bool` test on a document with nothing to record.
    pub(crate) fn record_svg_roots(&mut self) {
        if !self.svg_roots.dirty {
            return;
        }
        self.svg_roots.dirty = false;
        let dirty: Vec<NodeId> = self
            .svg_roots
            .entries
            .iter()
            .filter_map(|(&id, root)| root.dirty.then_some(id))
            .collect();
        for id in dirty {
            let Some(node) = self.get(id) else {
                self.svg_roots.remove(id);
                continue;
            };
            if node
                .flat_parent()
                .is_some_and(|parent| parent.is_element() && parent.in_svg_namespace())
            {
                self.svg_roots.remove(id);
                continue;
            }
            if !self.is_connected(id) || node.layout_computed_style().is_none() {
                self.svg_roots.dirty = true;
                continue;
            }
            let mut recorded = record(self, id, RootGroup::Box);
            shape(&mut recorded, &mut self.layout_state_mut().text_context);
            self.keep_svg_recording(id, recorded);
        }
    }

    /// Files `recording`, its text shaped, as what the outermost `svg`
    /// element `id` draws, and clears its mark.
    pub(crate) fn keep_svg_recording(&mut self, id: NodeId, recording: Recording) {
        let entry = self.svg_roots.entries.entry(id).or_default();
        entry.recording = Some(recording);
        entry.dirty = false;
        #[cfg(test)]
        {
            entry.records += 1;
        }
    }

    /// Whether the document has an outermost `svg` element filed: the one
    /// test a document without one pays where the plan meets a leaf box.
    #[inline]
    pub(crate) fn has_svg_roots(&self) -> bool {
        !self.svg_roots.is_empty()
    }

    /// What the outermost `svg` element `id` draws, as last recorded.
    #[inline]
    pub(crate) fn svg_recording(&self, id: NodeId) -> Option<&Recording> {
        if self.svg_roots.is_empty() {
            return None;
        }
        self.svg_roots.entries.get(&id)?.recording.as_ref()
    }

    /// Where `recording`, the outermost `svg` element `id`'s, draws in the
    /// element's border box, or `None` when it draws nothing: an empty
    /// recording or viewport, an empty content box or destination, or an
    /// element with no style or layout.
    pub(crate) fn svg_root_placement(
        &self,
        id: NodeId,
        recording: &Recording,
    ) -> Option<RootPlacement> {
        let (width, height) = recording.viewport;
        if recording.items.is_empty()
            || !(width > 0.0 && height > 0.0 && width.is_finite() && height.is_finite())
        {
            return None;
        }
        let style = self.paint_style(id)?;
        let layout = self.rounded_layout(id)?;
        let size = euclid::default::Size2D::new(layout.size.width, layout.size.height);
        let fragment = BoxFragment::new(
            Affine::IDENTITY,
            size,
            resolve_corner_radii(style, size),
            layout,
        );
        let destination = replaced_destination(style, &fragment, self.natural_size(id))?;
        let content = fragment.content_box;
        let radii = content_radii(&fragment);
        let clips = match fill_plan(&BoxShape::new(content, &radii), destination) {
            FillPlan::None => return None,
            FillPlan::Shape => SmallVec::from_iter([(content, radii)]),
            FillPlan::Rect(both) => SmallVec::from_iter([(both, CornerRadii::ZERO)]),
            FillPlan::Clipped(draw) => {
                SmallVec::from_iter([(content, radii), (draw, CornerRadii::ZERO)])
            }
        };
        let map = Affine::translate(destination.origin().to_vec2())
            * aspect_transform(recording.viewport, recording.aspect, destination.size());
        Some(RootPlacement { map, clips })
    }

    /// How many times the outermost `svg` element `root` was recorded.
    #[cfg(test)]
    pub(crate) fn svg_records(&self, root: NodeId) -> u32 {
        self.svg_roots
            .entries
            .get(&root)
            .map_or(0, |root| root.records)
    }

    /// Takes a freed node out of the table. One test on a document with no
    /// `svg` root.
    #[inline]
    pub(crate) fn forget_svg_root(&mut self, id: NodeId) {
        if !self.svg_roots.is_empty() {
            self.svg_roots.remove(id);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crate::render::image::{AspectAlign, AspectRatio};
    use crate::test_common::Doc;
    use crate::tree::document::NodeId;
    use crate::tree::xml::{XmlMarkup, XmlNamespace};

    const PAGE: &str =
        "page { display: flex; align-items: flex-start; width: 400px; height: 300px; }";

    const INLINE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"
        viewBox="0 0 10 10" preserveAspectRatio="xMinYMax slice">
      <rect id="r" width="10" height="10" fill="red"/>
    </svg>"#;

    /// Builds `markup` under the document element of a page with the SVG
    /// user-agent sheet, and answers its root.
    fn inline_root(doc: &mut Doc, markup: &str) -> NodeId {
        doc.dom.add_svg_user_agent_stylesheet();
        let markup = XmlMarkup::parse(markup.as_bytes(), Some(XmlNamespace::Svg))
            .expect("the markup parses");
        doc.dom.append_svg_markup(doc.root, &markup, || ())
    }

    /// How many times `root` was recorded.
    fn records(doc: &Doc, root: NodeId) -> u32 {
        doc.dom.svg_records(root)
    }

    /// Commits `doc` and answers how many times `root` has been recorded;
    /// the frame draws no vector image.
    fn commit_records(doc: &mut Doc, root: NodeId) -> u32 {
        let frame = doc.dom.commit();
        assert!(!frame.draws_vectors(), "an inline root bakes no texture");
        records(doc, root)
    }

    fn by_id(doc: &Doc, id: &str) -> NodeId {
        doc.dom
            .query_selector(doc.root, &format!("#{id}"))
            .expect("parses")
            .expect("an element with that id")
    }

    #[test]
    fn an_inline_root_keeps_its_recording_and_draws_no_vector_image() {
        let mut doc = Doc::with_css(PAGE);
        let root = inline_root(&mut doc, INLINE);
        assert_eq!(commit_records(&mut doc, root), 1);
        let recording = doc.dom.svg_recording(root).expect("recorded");
        assert_eq!(recording.viewport, (10.0, 10.0), "the viewBox size");
        assert_eq!(
            recording.aspect,
            AspectRatio {
                align: Some((AspectAlign::Min, AspectAlign::Max)),
                slice: true,
            }
        );
    }

    #[test]
    fn a_change_under_the_root_records_it_again_and_nothing_else_does() {
        let mut doc = Doc::with_css(&format!(
            "@namespace svg url(http://www.w3.org/2000/svg); {PAGE} \
             svg|rect.on {{ fill: rgb(0, 0, 255); }}"
        ));
        let svg = inline_root(&mut doc, INLINE);
        let rect = by_id(&doc, "r");
        let first = commit_records(&mut doc, svg);
        assert_eq!(
            commit_records(&mut doc, svg),
            first,
            "no change records nothing"
        );
        assert!(!doc.dom.needs_render(), "and builds no frame");

        doc.dom.set_attribute(rect, "width", "5");
        let attribute = commit_records(&mut doc, svg);
        assert_eq!(attribute, first + 1, "an attribute change records again");

        let circle = doc.dom.create_svg_element("circle", ());
        doc.dom.set_attribute(circle, "r", "2");
        doc.dom.append_child(svg, circle);
        let insertion = commit_records(&mut doc, svg);
        assert_eq!(insertion, attribute + 1, "a child insertion records again");

        doc.dom.set_attribute(rect, "class", "on");
        let class = commit_records(&mut doc, svg);
        assert_eq!(
            class,
            insertion + 1,
            "a class flip that changes `fill` records again"
        );
        assert_eq!(commit_records(&mut doc, svg), class);
    }

    /// A change elsewhere on the page builds a new frame, which plans and
    /// paints the root's recording again, but does not record it again.
    #[test]
    fn a_change_elsewhere_does_not_record_the_root_again() {
        let mut doc = Doc::with_css(&format!("{PAGE} view {{ width: 10px; height: 10px; }}"));
        let svg = inline_root(&mut doc, INLINE);
        let view = doc.el(doc.root, "view");
        let first = commit_records(&mut doc, svg);
        for value in ["a", "b", "c"] {
            doc.dom.set_attribute(view, "data-x", value);
            assert!(doc.dom.needs_render());
            assert_eq!(commit_records(&mut doc, svg), first, "{value}");
        }
    }

    #[test]
    fn freeing_the_root_empties_the_side_table() {
        let mut doc = Doc::with_css(PAGE);
        assert!(doc.dom.svg_roots.is_empty());
        let svg = inline_root(&mut doc, INLINE);
        doc.dom.commit();
        assert_eq!(doc.dom.svg_roots.entries.len(), 1);
        doc.dom.drop_subtree(svg);
        assert!(doc.dom.svg_roots.is_empty());
        assert_eq!(
            doc.dom.svg_roots.entries.capacity(),
            0,
            "and gives its storage back"
        );
        let frame = doc.dom.commit();
        assert!(frame.vector_draws().is_empty(), "nothing draws");
        assert!(
            frame
                .order
                .items()
                .iter()
                .all(|item| !matches!(item.kind, crate::visual::PaintItemKind::Svg { .. })),
            "and no recorded item is planned"
        );
    }

    /// The fill colour of the first path `root` last recorded, as RGBA8.
    fn recorded_fill(doc: &Doc, root: NodeId) -> [u8; 4] {
        let recording = doc.dom.svg_recording(root).expect("recorded");
        let Some(crate::svg::recording::Item::Path {
            fill: Some(fill), ..
        }) = recording.items.first()
        else {
            panic!("one filled path: {:?}", recording.items);
        };
        let crate::peniko::Brush::Solid(color) = fill.brush else {
            panic!("a solid fill: {:?}", fill.brush);
        };
        color.to_rgba8().to_u8_array()
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// A page whose `view#v` holds a root with one `rect#r.a`, committed
    /// once; the rect is red, and blue once its `class` is `b` or its
    /// `style` attribute says so. Answers the view, the root and the rect.
    fn hideable_page() -> (Doc, NodeId, NodeId, NodeId) {
        let mut doc = Doc::with_css(&format!(
            "{SVG_NS} {PAGE} svg|rect.a {{ fill: rgb(255, 0, 0); }} \
             svg|rect.b {{ fill: rgb(0, 0, 255); }}"
        ));
        let view = doc.el(doc.root, "view#v");
        doc.dom.add_svg_user_agent_stylesheet();
        let markup = XmlMarkup::parse(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
                <rect id="r" class="a" width="10" height="10"/></svg>"#,
            Some(XmlNamespace::Svg),
        )
        .expect("parses");
        let root = doc.dom.append_svg_markup(view, &markup, || ());
        let rect = by_id(&doc, "r");
        assert_eq!(commit_records(&mut doc, root), 1);
        assert_eq!(recorded_fill(&doc, root), RED);
        (doc, view, root, rect)
    }

    /// Hides the root's HTML ancestor, applies `write` to the rect, shows
    /// the ancestor again, and asserts the root recorded the change.
    fn assert_shown_root_records(write: fn(&mut Doc, NodeId)) {
        let (mut doc, view, root, rect) = hideable_page();
        doc.dom.set_attribute(view, "style", "display: none");
        let hidden = commit_records(&mut doc, root);
        write(&mut doc, rect);
        commit_records(&mut doc, root);
        doc.dom.set_attribute(view, "style", "");
        assert!(
            commit_records(&mut doc, root) > hidden,
            "the root records again once shown"
        );
        assert_eq!(recorded_fill(&doc, root), BLUE, "with the new fill");
    }

    /// A root under a `display: none` HTML ancestor loses its style; the
    /// flush that styles it again marks it, so a `class` or `style` write
    /// made while it was hidden, which marks nothing by itself, is drawn.
    #[test]
    fn a_class_change_while_hidden_draws_once_shown() {
        assert_shown_root_records(|doc, rect| doc.dom.set_attribute(rect, "class", "b"));
    }

    #[test]
    fn a_style_change_while_hidden_draws_once_shown() {
        assert_shown_root_records(|doc, rect| {
            doc.dom.set_attribute(rect, "style", "fill: rgb(0, 0, 255)");
        });
    }

    /// Removes `removed` (the root or its HTML parent) from the document,
    /// changes the rect's `class` while it is detached, inserts it again,
    /// and asserts the root recorded the change.
    fn assert_inserted_root_records(removed: fn(NodeId, NodeId) -> NodeId) {
        let (mut doc, view, root, rect) = hideable_page();
        let removed = removed(view, root);
        let parent = doc
            .dom
            .get(removed)
            .and_then(crate::tree::node::Node::parent_id);
        let parent = parent.expect("attached");
        doc.dom.remove_element(removed);
        let detached = commit_records(&mut doc, root);
        doc.dom.set_attribute(rect, "class", "b");
        commit_records(&mut doc, root);
        doc.dom.append_child(parent, removed);
        assert!(
            commit_records(&mut doc, root) > detached,
            "the root records again once inserted"
        );
        assert_eq!(recorded_fill(&doc, root), BLUE, "with the new fill");
    }

    /// Inserting an SVG element marks its root.
    #[test]
    fn a_class_change_while_detached_draws_once_inserted() {
        assert_inserted_root_records(|_, root| root);
    }

    /// Inserting an HTML element marks nothing below it; the flush that
    /// styles the root again does.
    #[test]
    fn a_class_change_while_an_ancestor_is_detached_draws_once_inserted() {
        assert_inserted_root_records(|view, _| view);
    }

    /// A laid-out page whose `svg` root holds a `path` that a class flip
    /// recolours; answers the root and the path.
    fn page() -> (Doc, NodeId, NodeId) {
        let mut doc = Doc::with_css(
            "@namespace svg url(http://www.w3.org/2000/svg);
             page { display: flex; align-items: flex-start; width: 400px; height: 300px; }
             svg|path.on { fill: rgb(0, 0, 255); }",
        );
        doc.dom.add_svg_user_agent_stylesheet();
        let markup = XmlMarkup::parse(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
                <path id="p" d="M0 0 H10 V10 Z" fill="red"/></svg>"#,
            Some(XmlNamespace::Svg),
        )
        .expect("parses");
        let root = doc.dom.append_svg_markup(doc.root, &markup, || ());
        let path = doc
            .dom
            .query_selector(doc.root, "#p")
            .expect("parses")
            .expect("the path");
        doc.dom.commit();
        assert_eq!(doc.dom.layout_cache_is_empty(root), Some(false), "laid out");
        (doc, root, path)
    }

    #[test]
    fn a_fill_change_below_the_root_invalidates_no_layout() {
        let (mut doc, root, path) = page();
        let before = records(&doc, root);
        doc.dom.set_attribute(path, "class", "on");
        doc.dom.flush_styles_with_damage_sink(&mut |_, _| {});
        assert!(!doc.dom.layout_owed(), "no layout pass owed");
        assert_eq!(
            doc.dom.layout_cache_is_empty(root),
            Some(false),
            "the root keeps its box cache"
        );
        assert_eq!(
            commit_records(&mut doc, root),
            before + 1,
            "the root records again"
        );
    }

    #[test]
    fn a_width_change_on_the_root_relays_it_out() {
        let (mut doc, root, _) = page();
        doc.dom.set_attribute(root, "width", "60");
        doc.dom.flush_styles_with_damage_sink(&mut |_, _| {});
        assert!(doc.dom.layout_owed());
        assert_eq!(doc.dom.layout_cache_is_empty(root), Some(true));
        doc.dom.commit();
        let width = doc.dom.rounded_layout(root).expect("laid out").size.width;
        assert!((width - 60.0).abs() < f32::EPSILON, "width {width}");
    }

    // --- CSS animations inside an inline root ---------------------------

    const SVG_NS: &str = "@namespace svg url(http://www.w3.org/2000/svg);";

    /// A page whose inline root holds `body`, styled by `css` (after the SVG
    /// namespace rule), committed once.
    fn animated_page(css: &str, body: &str) -> Doc {
        let mut doc = Doc::with_css(&format!("{SVG_NS} {PAGE} {css}"));
        inline_root(
            &mut doc,
            &format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40">{body}</svg>"#
            ),
        );
        doc.dom.commit();
        doc
    }

    /// How many times the page's root has been recorded after a commit, and
    /// whether the frame asks the host for further ticks.
    fn commit(doc: &mut Doc) -> (u32, bool) {
        let root = root_of(doc);
        let frame = doc.dom.commit();
        assert!(!frame.draws_vectors(), "an inline root bakes no texture");
        (records(doc, root), frame.needs_main_ticks())
    }

    /// Runs a finite one-second animation from 0 s to past its end: each
    /// tick inside it restyles, re-records and keeps frames coming; the end
    /// stops them and the recording stays.
    fn assert_animates_then_ends(mut doc: Doc) {
        assert!(doc.dom.has_active_animations(), "running after the flush");
        doc.dom.advance_animations(0.0);
        let mut previous = commit(&mut doc).0;
        for now in [0.25, 0.5, 0.75] {
            let tick = doc.dom.advance_animations(now);
            assert!(
                tick.restyled > 0 && tick.needs_next_frame,
                "{now}: {tick:?}"
            );
            let (recorded, ticks) = commit(&mut doc);
            assert!(recorded > previous, "{now}: the root records again");
            assert!(ticks, "{now}: the frame asks for the next tick");
            previous = recorded;
        }
        let tick = doc.dom.advance_animations(2.0);
        assert!(!tick.needs_next_frame, "{tick:?}");
        assert!(!doc.dom.has_active_animations());
        let (ended, ticks) = commit(&mut doc);
        assert!(!ticks, "an ended animation asks for nothing");
        doc.dom.advance_animations(3.0);
        assert_eq!(commit(&mut doc).0, ended, "nothing records once it ended");
    }

    #[test]
    fn a_fill_animation_records_the_root_each_tick() {
        assert_animates_then_ends(animated_page(
            "@keyframes paint { from { fill: rgb(255, 0, 0); } to { fill: rgb(0, 0, 255); } }
             svg|rect { animation: paint 1s linear; }",
            r#"<rect width="40" height="40"/>"#,
        ));
    }

    #[test]
    fn a_dash_offset_animation_records_the_root_each_tick() {
        assert_animates_then_ends(animated_page(
            "@keyframes dash { from { stroke-dashoffset: 0; } to { stroke-dashoffset: 20px; } }
             svg|path { animation: dash 1s linear; }",
            r#"<path d="M0 20 H40" stroke="black" stroke-width="4" stroke-dasharray="5 5"/>"#,
        ));
    }

    /// `opacity` is a composite property: the painter runs a finite fade of
    /// a shape below the root, and the frame asks the main thread for no
    /// tick. A tick the main thread runs all the same records the root
    /// again; the one that ends the fade records it at the base value, with
    /// no layer.
    #[test]
    fn an_opacity_animation_below_the_root_runs_in_the_painter() {
        let mut doc = animated_page(
            "@keyframes fade { from { opacity: 1; } to { opacity: 0.2; } }
             svg|rect { animation: fade 1s linear; }",
            r#"<rect width="40" height="40" fill="red"/>"#,
        );
        doc.dom.advance_animations(0.0);
        let (mut previous, _) = commit(&mut doc);
        for now in [0.25, 0.5, 0.75] {
            let tick = doc.dom.advance_animations(now);
            assert!(
                tick.restyled > 0 && tick.needs_next_frame,
                "{now}: {tick:?}"
            );
            let (recorded, ticks) = commit(&mut doc);
            assert!(recorded > previous, "{now}: the tick records the root");
            assert!(!ticks, "{now}: the painter runs the fade");
            previous = recorded;
        }
        let tick = doc.dom.advance_animations(2.0);
        assert!(!tick.needs_next_frame, "{tick:?}");
        let (ended, ticks) = commit(&mut doc);
        assert!(ended > previous, "the end records the root");
        assert!(!ticks);
        let kept = doc.dom.svg_recording(root_of(&doc)).expect("recorded");
        assert!(
            matches!(
                kept.items.as_slice(),
                [crate::svg::recording::Item::Path { .. }]
            ),
            "{:?}",
            kept.items
        );
    }

    /// The spinner: a group turning about the centre of its own box, run by
    /// the painter. A tick the main thread runs all the same records the
    /// root again at the value the cascade reached.
    #[test]
    fn a_spinning_group_runs_in_the_painter() {
        let mut doc = animated_page(
            "@keyframes spin { to { transform: rotate(360deg); } }
             svg|g { transform-origin: center; transform-box: fill-box;
                     animation: spin 1s linear infinite; }",
            r#"<g><rect x="10" y="10" width="20" height="10" fill="red"/></g>"#,
        );
        doc.dom.advance_animations(0.0);
        let (mut previous, _) = commit(&mut doc);
        for now in [0.25, 0.5, 1.75, 30.1] {
            let tick = doc.dom.advance_animations(now);
            assert!(
                tick.restyled > 0 && tick.needs_next_frame,
                "{now}: {tick:?}"
            );
            let (recorded, ticks) = commit(&mut doc);
            assert!(recorded > previous, "{now}: the tick records the root");
            assert!(!ticks, "{now}: the painter runs the turn");
            previous = recorded;
        }
        // A quarter turn about the rect's centre (20, 15).
        doc.dom.advance_animations(30.25);
        doc.dom.commit();
        let kept = doc.dom.svg_recording(root_of(&doc)).expect("recorded");
        let Some(crate::svg::recording::Item::Path { transform, .. }) = kept.items.first() else {
            panic!("one path");
        };
        let centre = crate::kurbo::Point::new(20.0, 15.0);
        let corner = *transform * crate::kurbo::Point::new(10.0, 10.0);
        let turned = centre + crate::kurbo::Vec2::new(5.0, -10.0);
        assert!(
            corner.distance(turned) < 0.05,
            "{corner:?} against {turned:?}"
        );
    }

    #[test]
    fn a_fill_transition_records_the_root_each_tick() {
        let mut doc = animated_page(
            "svg|rect { fill: rgb(255, 0, 0); transition: fill 1s linear; }
             svg|rect.on { fill: rgb(0, 0, 255); }",
            r#"<rect id="t" width="40" height="40"/>"#,
        );
        assert!(!doc.dom.has_active_animations());
        let rect = by_id(&doc, "t");
        doc.dom.set_attribute(rect, "class", "on");
        doc.dom.commit();
        assert_animates_then_ends(doc);
    }

    fn root_of(doc: &Doc) -> NodeId {
        doc.dom
            .query_selector(doc.root, "svg")
            .expect("parses")
            .expect("the root")
    }
}
