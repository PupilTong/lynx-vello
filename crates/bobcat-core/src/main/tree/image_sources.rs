//! The sources of the document's `image` and `svg` elements, which of them
//! each element presents, and what their settling owes script.
//!
//! `terminus` offers two things here and knows nothing of the elements: a
//! replaced box presents one picture at a natural size
//! ([`terminus::link::Document::set_picture`]), and a URL can be asked for
//! and its status learnt ([`terminus::link::Document::request_image`],
//! [`terminus::link::Document::apply_image_events`]). An `image` element's
//! two sources, the choice between them, its `load` and `error`, and the
//! playback of an animated `src` are this module's.
//!
//! # Two sources, one picture
//!
//! An `image` names up to two sources: `src`, the picture it is for, and
//! `placeholder`, the picture it shows until `src` has pixels. They are
//! independent requests made at the same time — a placeholder is not what
//! the element reaches for once its source fails (ruled 2026-09-17,
//! `docs/tracking/media-resources.md`). The element presents its `src` while
//! that is loaded, its `placeholder` while that is loaded and `src` is not,
//! and otherwise whichever it names, which draws nothing until it loads. So a
//! loaded `src` suppresses the placeholder for good, including one whose
//! pixels land later: a source's status never moves once settled. The
//! natural size presented is always that of the picture presented, because
//! `object-fit` resolves one against the other.
//!
//! An `svg` names one source, `src` or the synthetic source its `content` is
//! filed under ([`terminus::link::Document::insert_image_document`]), and
//! presents it from the moment it is named.
//!
//! # An animated `src`
//!
//! A URL picture of an animated source plays under the count its file
//! states, on one playhead every draw of the URL shares, with no control: a
//! CSS image, and an `image`'s `placeholder`. An `image` whose own `src`
//! loads animated ([`ImageStatus::Bitmap`]'s `animated`) presents a picture
//! of its own instead ([`PictureKey::Id`]) and asks the host to play the
//! source's frames into it under a playhead of its own
//! ([`PlaybackRequest::Play`]), with the policy its `loop-count` and
//! `autoplay` give when the entry that loaded it ends: a later change of
//! either takes effect at the next load. Read then rather than at the load
//! itself because a `src` this document has already seen settles inside the
//! `__SetAttribute` that writes it, which may come before the write of
//! either attribute in the same batch. A `src` that changes ends the picture
//! ([`PlaybackRequest::End`]), so the next one starts at the first frame,
//! with no loop completed and not paused; so does a free. The element's UI
//! methods send the four controls ([`Self::control`]), and the host's loop
//! reports come back with the image reports ([`apply_image_events`]).
//!
//! The requests are queued here and handed to the host by the entry's
//! epilogue ([`Self::take_playback`]); no frame and no pixel is ever on this
//! thread.
//!
//! # Outcomes
//!
//! Only an element's own `src` produces an [`ImageOutcome`], which the
//! runtime turns into an event. A placeholder is an interim picture the page
//! did not ask about, so neither of its endings is an event, and its
//! animation fires nothing; the type has no variant naming one.
//!
//! A source settles in one of two places: at the write that names it, when
//! the document has already seen it settle (a second mount of a URL, which
//! no report will name again), or when a report settles it
//! ([`apply_image_events`]). The first happens inside the `__SetAttribute`
//! that wrote the attribute, so the components queue it rather than
//! dispatch it; the second is queued by the runtime for the same epilogue.
//! An animated `src` owes [`ImageOutcome::StartPlay`] before its `load`, in
//! both places, which is a `startplay` when the policy its picture was
//! played under plays from the start ([`ImageSources::autoplays`]).
//!
//! # Lifetime
//!
//! The record of an element goes when the element is freed
//! ([`terminus::link::CustomElement::freed`]), so a report that lands after
//! a free names no stale id, and the picture it played ends. The replaced
//! box's own hold on what it presents is `terminus`'s, released by the same
//! free.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use terminus::link::{
    DocumentKind, ImageEvent, ImageStatus, Loops, NodeId, PictureId, PictureKey, PlayPolicy,
    PlaybackControl, PlaybackReport, PlaybackRequest,
};

use super::LynxDocument;

pub(crate) const LOOP_COUNT_ATTRIBUTE: &str = "loop-count";
pub(crate) const AUTOPLAY_ATTRIBUTE: &str = "autoplay";

pub(crate) const START_PLAY_EVENT: &str = "startplay";
pub(crate) const CURRENT_LOOP_COMPLETE_EVENT: &str = "currentloopcomplete";
pub(crate) const FINAL_LOOP_COMPLETE_EVENT: &str = "finalloopcomplete";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageRole {
    Source,
    Placeholder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageOutcome {
    Loaded {
        node: NodeId,
        width: u32,
        height: u32,
    },
    Failed {
        node: NodeId,
    },
    StartPlay {
        node: NodeId,
        picture: PictureId,
    },
    Played {
        node: NodeId,
        name: &'static str,
    },
}

impl ImageOutcome {
    fn of(node: NodeId, status: ImageStatus) -> Option<Self> {
        match status {
            ImageStatus::Pending => None,
            ImageStatus::Failed => Some(Self::Failed { node }),
            ImageStatus::Bitmap { width, height, .. } | ImageStatus::Vector { width, height } => {
                Some(Self::Loaded {
                    node,
                    width,
                    height,
                })
            }
        }
    }
}

pub(crate) type ImageOutcomes = SmallVec<[ImageOutcome; 2]>;

pub(crate) fn play_policy(loop_count: Option<&str>, autoplay: Option<&str>) -> PlayPolicy {
    let plays = loop_count
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|plays| plays.is_finite() && *plays >= 1.0)
        .map_or(0, |plays| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a finite count of at least one, truncated and saturating at u32::MAX"
            )]
            let plays = plays as u32;
            plays
        });
    PlayPolicy {
        loops: Loops::from_plays(plays),
        paused: autoplay.is_some_and(|value| value == "false"),
    }
}

#[derive(Debug, Default)]
struct Named {
    source: Option<Arc<str>>,
    placeholder: Option<Arc<str>>,
    playable: bool,
    playing: Option<PictureId>,
}

#[derive(Debug)]
enum Queued {
    Start {
        element: NodeId,
        picture: PictureId,
        source: Arc<str>,
    },
    Request(PlaybackRequest),
}

impl Named {
    fn get(&self, role: ImageRole) -> Option<&Arc<str>> {
        match role {
            ImageRole::Source => self.source.as_ref(),
            ImageRole::Placeholder => self.placeholder.as_ref(),
        }
    }

    fn slot(&mut self, role: ImageRole) -> &mut Option<Arc<str>> {
        match role {
            ImageRole::Source => &mut self.source,
            ImageRole::Placeholder => &mut self.placeholder,
        }
    }
}

#[derive(Debug, Default)]
struct State {
    elements: FxHashMap<NodeId, Named>,
    holders: FxHashMap<Arc<str>, SmallVec<[(NodeId, ImageRole); 1]>>,
    players: FxHashMap<PictureId, NodeId>,
    playback: Vec<Queued>,
    autoplaying: rustc_hash::FxHashSet<PictureId>,
}

impl State {
    fn unhold(&mut self, source: &str, holder: (NodeId, ImageRole)) {
        if let Some(holders) = self.holders.get_mut(source) {
            holders.retain(|held| *held != holder);
            if holders.is_empty() {
                self.holders.remove(source);
            }
        }
    }

    fn stop_playing(&mut self, element: NodeId) {
        let Some(picture) = self
            .elements
            .get_mut(&element)
            .and_then(|named| named.playing.take())
        else {
            return;
        };
        self.players.remove(&picture);
        self.autoplaying.remove(&picture);
        self.playback
            .push(Queued::Request(PlaybackRequest::End { picture }));
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ImageSources(Rc<RefCell<State>>);

impl ImageSources {
    pub(crate) fn set(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        role: ImageRole,
        value: Option<&str>,
    ) -> Option<ImageOutcome> {
        self.name(document, element, role, value, false)
            .into_iter()
            .next()
    }

    pub(crate) fn set_playable(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        role: ImageRole,
        value: Option<&str>,
    ) -> ImageOutcomes {
        self.name(document, element, role, value, true)
    }

    fn name(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        role: ImageRole,
        value: Option<&str>,
        playable: bool,
    ) -> ImageOutcomes {
        let held = self
            .0
            .borrow()
            .elements
            .get(&element)
            .and_then(|named| named.get(role).cloned());
        if held.as_deref() == value {
            return ImageOutcomes::new();
        }
        let status = value.map(|value| document.request_image(value));
        {
            let mut state = self.0.borrow_mut();
            if let Some(previous) = &held {
                state.unhold(previous, (element, role));
            }
            if role == ImageRole::Source {
                state.stop_playing(element);
            }
            let value = value.map(Arc::<str>::from);
            if let Some(value) = &value {
                state
                    .holders
                    .entry(Arc::clone(value))
                    .or_default()
                    .push((element, role));
            }
            let named = state.elements.entry(element).or_default();
            named.playable |= playable;
            *named.slot(role) = value;
            if named.source.is_none() && named.placeholder.is_none() {
                state.elements.remove(&element);
            }
        }
        let mut outcomes = ImageOutcomes::new();
        if let (ImageRole::Source, Some(status)) = (role, status) {
            if let Some(picture) = self.start_playing(element, status) {
                outcomes.push(ImageOutcome::StartPlay {
                    node: element,
                    picture,
                });
            }
            outcomes.extend(ImageOutcome::of(element, status));
        }
        self.present(document, element);
        outcomes
    }

    pub(crate) fn set_document(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        bytes: &[u8],
        kind: DocumentKind,
    ) -> Option<ImageOutcome> {
        let source = document.insert_image_document(bytes, kind);
        self.set(document, element, ImageRole::Source, Some(&source))
    }

    pub(crate) fn settle(
        &self,
        document: &mut LynxDocument,
        settled: &[(Arc<str>, ImageStatus)],
    ) -> Vec<ImageOutcome> {
        let mut outcomes = Vec::new();
        for (source, status) in settled {
            let holders = self
                .0
                .borrow()
                .holders
                .get(source)
                .cloned()
                .unwrap_or_default();
            for (node, role) in holders {
                if role == ImageRole::Source {
                    if let Some(picture) = self.start_playing(node, *status) {
                        outcomes.push(ImageOutcome::StartPlay { node, picture });
                    }
                    if let Some(outcome) = ImageOutcome::of(node, *status) {
                        outcomes.push(outcome);
                    }
                }
                self.present(document, node);
            }
        }
        outcomes
    }

    fn played(&self, events: &[ImageEvent]) -> Vec<ImageOutcome> {
        let state = self.0.borrow();
        events
            .iter()
            .filter_map(|event| match event {
                ImageEvent::Playback { picture, report } => {
                    let node = *state.players.get(picture)?;
                    let name = match report {
                        PlaybackReport::LoopComplete => CURRENT_LOOP_COMPLETE_EVENT,
                        PlaybackReport::FinalLoopComplete => FINAL_LOOP_COMPLETE_EVENT,
                        PlaybackReport::Media(_) => return None,
                    };
                    Some(ImageOutcome::Played { node, name })
                }
                ImageEvent::Loaded { .. }
                | ImageEvent::ParsedDocument { .. }
                | ImageEvent::Failed { .. } => None,
            })
            .collect()
    }

    fn start_playing(&self, element: NodeId, status: ImageStatus) -> Option<PictureId> {
        if !matches!(status, ImageStatus::Bitmap { animated: true, .. }) {
            return None;
        }
        let mut state = self.0.borrow_mut();
        let named = state.elements.get_mut(&element)?;
        if !named.playable || named.playing.is_some() {
            return None;
        }
        let source = named.source.clone()?;
        let picture = PictureId::new();
        named.playing = Some(picture);
        state.players.insert(picture, element);
        state.playback.push(Queued::Start {
            element,
            picture,
            source,
        });
        Some(picture)
    }

    pub(crate) fn autoplays(&self, picture: PictureId) -> bool {
        self.0.borrow().autoplaying.contains(&picture)
    }

    pub(crate) fn control(&self, element: NodeId, control: PlaybackControl) -> bool {
        let mut state = self.0.borrow_mut();
        let Some(picture) = state.elements.get(&element).and_then(|named| named.playing) else {
            return false;
        };
        state
            .playback
            .push(Queued::Request(PlaybackRequest::Control {
                picture,
                control,
            }));
        true
    }

    pub(crate) fn take_playback(&self, document: &LynxDocument) -> Vec<PlaybackRequest> {
        let mut state = self.0.borrow_mut();
        if state.playback.is_empty() {
            return Vec::new();
        }
        let queued = std::mem::take(&mut state.playback);
        queued
            .into_iter()
            .map(|queued| match queued {
                Queued::Request(request) => request,
                Queued::Start {
                    element,
                    picture,
                    source,
                } => {
                    let node = document.get(element);
                    let attribute = |name: &str| node.and_then(|node| node.attribute(name));
                    let policy = play_policy(
                        attribute(LOOP_COUNT_ATTRIBUTE).as_deref(),
                        attribute(AUTOPLAY_ATTRIBUTE).as_deref(),
                    );
                    if !policy.paused && state.players.contains_key(&picture) {
                        state.autoplaying.insert(picture);
                    }
                    PlaybackRequest::Play {
                        picture,
                        source,
                        policy,
                    }
                }
            })
            .collect()
    }

    pub(crate) fn forget(&self, element: NodeId) {
        let mut state = self.0.borrow_mut();
        state.stop_playing(element);
        let Some(named) = state.elements.remove(&element) else {
            return;
        };
        for role in [ImageRole::Source, ImageRole::Placeholder] {
            if let Some(source) = named.get(role) {
                state.unhold(source, (element, role));
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn source(&self, element: NodeId, role: ImageRole) -> Option<Arc<str>> {
        self.0
            .borrow()
            .elements
            .get(&element)
            .and_then(|named| named.get(role).cloned())
    }

    #[cfg(test)]
    pub(crate) fn playing(&self, element: NodeId) -> Option<PictureId> {
        self.0
            .borrow()
            .elements
            .get(&element)
            .and_then(|named| named.playing)
    }

    fn present(&self, document: &mut LynxDocument, element: NodeId) {
        let (source, placeholder, playing) = {
            let state = self.0.borrow();
            let named = state.elements.get(&element);
            (
                named.and_then(|named| named.source.clone()),
                named.and_then(|named| named.placeholder.clone()),
                named.and_then(|named| named.playing),
            )
        };
        let natural = |name: &Option<Arc<str>>| {
            name.as_deref()
                .and_then(|name| document.image_status(name).natural_size())
        };
        let presented = match (natural(&source), natural(&placeholder)) {
            (Some(size), _) => source.map(|source| {
                let key = playing.map_or(PictureKey::Url(source), PictureKey::Id);
                (key, Some(size))
            }),
            (None, Some(size)) => {
                placeholder.map(|placeholder| (PictureKey::Url(placeholder), Some(size)))
            }
            (None, None) => source
                .or(placeholder)
                .map(|name| (PictureKey::Url(name), None)),
        };
        document.set_picture(element, presented);
    }
}

pub(crate) fn apply_image_events(
    document: &mut LynxDocument,
    sources: &ImageSources,
    events: &[ImageEvent],
) -> Vec<ImageOutcome> {
    let settled = document.apply_image_events(events);
    let mut outcomes = sources.settle(document, &settled);
    outcomes.extend(sources.played(events));
    outcomes
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use std::sync::Arc;

    use terminus::link::{ImageEvent, ImageStatus, NodeId, PictureKey};

    use super::super::test_support::{child, with_image_sources};
    use super::super::{LynxDocument, PageConfig};
    use super::{ImageOutcome, ImageRole, ImageSources, apply_image_events};

    const SRC: &str = "app:///a.png";
    const OTHER_SRC: &str = "app:///b.png";
    const PLACEHOLDER: &str = "app:///p.png";

    fn images() -> (LynxDocument, ImageSources) {
        let (document, _, sources) = with_image_sources(PageConfig::default());
        (document, sources)
    }

    fn element(document: &mut LynxDocument) -> NodeId {
        child(document, "view", "position: absolute")
    }

    fn loaded(source: &str, width: u32, height: u32) -> ImageEvent {
        ImageEvent::Loaded {
            source: Arc::from(source),
            width,
            height,
            animated: false,
        }
    }

    fn failed(source: &str) -> ImageEvent {
        ImageEvent::Failed {
            source: Arc::from(source),
        }
    }

    fn presented(document: &LynxDocument, element: NodeId) -> Option<String> {
        document
            .picture(element)
            .and_then(PictureKey::url)
            .map(str::to_owned)
    }

    fn natural(document: &mut LynxDocument, element: NodeId) -> (f32, f32) {
        document.layout();
        let layout = document.rounded_layout(element).expect("laid out");
        (layout.size.width, layout.size.height)
    }

    #[test]
    fn a_loaded_source_suppresses_the_placeholder() {
        let (mut document, sources) = images();
        let image = element(&mut document);
        sources.set(&mut document, image, ImageRole::Source, Some(SRC));
        sources.set(
            &mut document,
            image,
            ImageRole::Placeholder,
            Some(PLACEHOLDER),
        );
        assert_eq!(
            natural(&mut document, image),
            (0.0, 0.0),
            "neither source has pixels yet, so there is no bitmap to describe"
        );

        apply_image_events(&mut document, &sources, &[loaded(PLACEHOLDER, 4, 4)]);
        assert_eq!(presented(&document, image).as_deref(), Some(PLACEHOLDER));
        assert_eq!(natural(&mut document, image), (4.0, 4.0));

        apply_image_events(&mut document, &sources, &[loaded(SRC, 40, 20)]);
        assert_eq!(presented(&document, image).as_deref(), Some(SRC));
        assert_eq!(
            natural(&mut document, image),
            (40.0, 20.0),
            "the element's own source takes over the moment it has pixels"
        );
    }

    #[test]
    fn a_placeholder_arriving_after_the_source_changes_nothing() {
        let (mut document, sources) = images();
        let image = element(&mut document);
        sources.set(&mut document, image, ImageRole::Source, Some(SRC));
        sources.set(
            &mut document,
            image,
            ImageRole::Placeholder,
            Some(PLACEHOLDER),
        );

        apply_image_events(&mut document, &sources, &[loaded(SRC, 40, 20)]);
        apply_image_events(&mut document, &sources, &[loaded(PLACEHOLDER, 4, 4)]);
        assert_eq!(presented(&document, image).as_deref(), Some(SRC));
        assert_eq!(natural(&mut document, image), (40.0, 20.0));
    }

    #[test]
    fn a_failed_source_leaves_the_placeholder_showing() {
        let (mut document, sources) = images();
        let image = element(&mut document);
        sources.set(&mut document, image, ImageRole::Source, Some(SRC));
        sources.set(
            &mut document,
            image,
            ImageRole::Placeholder,
            Some(PLACEHOLDER),
        );

        apply_image_events(&mut document, &sources, &[failed(SRC)]);
        assert_eq!(natural(&mut document, image), (0.0, 0.0));

        apply_image_events(&mut document, &sources, &[loaded(PLACEHOLDER, 4, 4)]);
        assert_eq!(presented(&document, image).as_deref(), Some(PLACEHOLDER));
        assert_eq!(natural(&mut document, image), (4.0, 4.0));
    }

    #[test]
    fn a_placeholder_alone_makes_an_element_replaced() {
        let (mut document, sources) = images();
        let image = element(&mut document);
        sources.set(
            &mut document,
            image,
            ImageRole::Placeholder,
            Some(PLACEHOLDER),
        );

        assert!(document.picture(image).is_some());
        apply_image_events(&mut document, &sources, &[loaded(PLACEHOLDER, 4, 4)]);
        assert_eq!(natural(&mut document, image), (4.0, 4.0));
    }

    #[test]
    fn swapping_the_source_drops_the_departed_bitmaps_size() {
        let (mut document, sources) = images();
        let image = element(&mut document);
        sources.set(&mut document, image, ImageRole::Source, Some(SRC));
        apply_image_events(&mut document, &sources, &[loaded(SRC, 40, 20)]);
        assert_eq!(natural(&mut document, image), (40.0, 20.0));

        sources.set(&mut document, image, ImageRole::Source, Some(OTHER_SRC));
        assert_eq!(
            natural(&mut document, image),
            (0.0, 0.0),
            "a pending source draws nothing, and describes nothing"
        );
        assert_eq!(presented(&document, image).as_deref(), Some(OTHER_SRC));

        sources.set(
            &mut document,
            image,
            ImageRole::Placeholder,
            Some(PLACEHOLDER),
        );
        apply_image_events(&mut document, &sources, &[loaded(PLACEHOLDER, 4, 4)]);
        assert_eq!(natural(&mut document, image), (4.0, 4.0));
    }

    #[test]
    fn naming_a_settled_source_reports_its_outcome_and_sizes_the_element() {
        let (mut document, sources) = images();
        let first = element(&mut document);
        assert_eq!(
            sources.set(&mut document, first, ImageRole::Source, Some(SRC)),
            None,
            "a pending source owes nothing yet"
        );
        apply_image_events(
            &mut document,
            &sources,
            &[loaded(SRC, 40, 20), failed(OTHER_SRC)],
        );

        let second = element(&mut document);
        assert_eq!(
            sources.set(&mut document, second, ImageRole::Source, Some(SRC)),
            Some(ImageOutcome::Loaded {
                node: second,
                width: 40,
                height: 20
            })
        );
        assert_eq!(
            natural(&mut document, second),
            (40.0, 20.0),
            "and it lays out in the commit that first draws it"
        );

        let third = element(&mut document);
        assert_eq!(
            sources.set(&mut document, third, ImageRole::Source, Some(OTHER_SRC)),
            Some(ImageOutcome::Failed { node: third })
        );
        assert_eq!(natural(&mut document, third), (0.0, 0.0));
    }

    #[test]
    fn rewriting_the_same_source_reports_nothing() {
        let (mut document, sources) = images();
        let image = element(&mut document);
        sources.set(&mut document, image, ImageRole::Source, Some(SRC));
        apply_image_events(&mut document, &sources, &[loaded(SRC, 40, 20)]);

        assert_eq!(
            sources.set(&mut document, image, ImageRole::Source, Some(SRC)),
            None
        );
    }

    #[test]
    fn a_report_names_only_the_elements_whose_own_source_settled() {
        let (mut document, sources) = images();
        let owner = element(&mut document);
        let borrower = element(&mut document);
        sources.set(&mut document, owner, ImageRole::Source, Some(SRC));
        sources.set(&mut document, borrower, ImageRole::Placeholder, Some(SRC));

        assert_eq!(
            apply_image_events(&mut document, &sources, &[loaded(SRC, 40, 20)]),
            vec![ImageOutcome::Loaded {
                node: owner,
                width: 40,
                height: 20
            }]
        );
        assert_eq!(
            natural(&mut document, borrower),
            (40.0, 20.0),
            "the placeholder still sizes the element drawing it"
        );

        let (mut document, sources) = images();
        let image = element(&mut document);
        sources.set(
            &mut document,
            image,
            ImageRole::Placeholder,
            Some(PLACEHOLDER),
        );
        assert!(
            apply_image_events(&mut document, &sources, &[failed(PLACEHOLDER)]).is_empty(),
            "a placeholder failing is nobody's event either"
        );

        let second = element(&mut document);
        assert_eq!(
            sources.set(
                &mut document,
                second,
                ImageRole::Placeholder,
                Some(PLACEHOLDER)
            ),
            None,
            "a settled placeholder is still nobody's event"
        );
    }

    #[test]
    fn a_failed_source_reports_its_elements() {
        let (mut document, sources) = images();
        let image = element(&mut document);
        sources.set(&mut document, image, ImageRole::Source, Some(SRC));

        assert_eq!(
            apply_image_events(&mut document, &sources, &[failed(SRC)]),
            vec![ImageOutcome::Failed { node: image }]
        );
        assert!(
            apply_image_events(&mut document, &sources, &[failed(SRC)]).is_empty(),
            "and a source reported twice moved nothing, so it owes nothing"
        );
    }

    #[test]
    fn a_zero_axis_load_is_owed_as_a_failure() {
        let (mut document, sources) = images();
        let image = element(&mut document);
        sources.set(&mut document, image, ImageRole::Source, Some(SRC));
        assert_eq!(
            apply_image_events(&mut document, &sources, &[loaded(SRC, 0, 4)]),
            vec![ImageOutcome::Failed { node: image }]
        );
    }

    #[test]
    fn one_url_in_both_roles_is_two_holders() {
        let (mut document, sources) = images();
        let image = element(&mut document);
        sources.set(&mut document, image, ImageRole::Source, Some(SRC));
        sources.set(&mut document, image, ImageRole::Placeholder, Some(SRC));
        assert_eq!(
            document.take_wanted_images().len(),
            1,
            "one URL, one request"
        );

        sources.set(&mut document, image, ImageRole::Source, None);
        assert!(
            apply_image_events(&mut document, &sources, &[loaded(SRC, 40, 20)]).is_empty(),
            "the placeholder owes nothing"
        );
        assert_eq!(presented(&document, image).as_deref(), Some(SRC));
        assert_eq!(natural(&mut document, image), (40.0, 20.0));
    }

    #[test]
    fn taking_the_last_source_off_an_element_unreplaces_it() {
        let (mut document, sources) = images();
        let image = element(&mut document);
        sources.set(&mut document, image, ImageRole::Source, Some(SRC));
        sources.set(
            &mut document,
            image,
            ImageRole::Placeholder,
            Some(PLACEHOLDER),
        );
        apply_image_events(
            &mut document,
            &sources,
            &[loaded(SRC, 40, 20), loaded(PLACEHOLDER, 4, 4)],
        );

        sources.set(&mut document, image, ImageRole::Source, None);
        assert!(document.picture(image).is_some());
        assert_eq!(
            natural(&mut document, image),
            (4.0, 4.0),
            "the placeholder is all that is left, and it is what is drawn"
        );

        sources.set(&mut document, image, ImageRole::Placeholder, None);
        assert!(document.picture(image).is_none());
        assert_eq!(document.picture(image), None);
        assert_eq!(sources.source(image, ImageRole::Placeholder), None);
    }

    #[test]
    fn a_freed_element_owes_nothing() {
        let (mut document, sources) = images();
        let image = child(&mut document, "image", "");
        document.set_attribute(image, "src", SRC);
        document.set_attribute(image, "placeholder", PLACEHOLDER);
        assert_eq!(
            sources.source(image, ImageRole::Source).as_deref(),
            Some(SRC)
        );
        document.drop_element(image);
        assert_eq!(sources.source(image, ImageRole::Source), None);

        assert!(
            apply_image_events(
                &mut document,
                &sources,
                &[loaded(SRC, 40, 20), loaded(PLACEHOLDER, 4, 4)]
            )
            .is_empty(),
            "a dropped element owes no events and has no size to set"
        );
    }

    #[test]
    fn freeing_one_element_leaves_a_sibling_on_the_same_source() {
        let (mut document, sources) = images();
        let leaving = child(&mut document, "image", "");
        let staying = child(&mut document, "image", "");
        for image in [leaving, staying] {
            document.set_attribute(image, "src", SRC);
        }

        document.drop_element(leaving);
        assert_eq!(
            apply_image_events(&mut document, &sources, &[loaded(SRC, 40, 20)]),
            vec![ImageOutcome::Loaded {
                node: staying,
                width: 40,
                height: 20
            }]
        );
        assert_eq!(presented(&document, staying).as_deref(), Some(SRC));
        assert_eq!(
            document.image_status(SRC),
            ImageStatus::Bitmap {
                width: 40,
                height: 20,
                animated: false,
            }
        );
    }
}
