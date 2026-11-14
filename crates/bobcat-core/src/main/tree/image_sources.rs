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

/// The `image` attribute giving how many times an animated `src` plays.
pub(crate) const LOOP_COUNT_ATTRIBUTE: &str = "loop-count";
/// The `image` attribute saying whether an animated `src` plays as it loads.
pub(crate) const AUTOPLAY_ATTRIBUTE: &str = "autoplay";

/// The events an `image` playing an animated `src` fires, each with an empty
/// detail.
pub(crate) const START_PLAY_EVENT: &str = "startplay";
pub(crate) const CURRENT_LOOP_COMPLETE_EVENT: &str = "currentloopcomplete";
pub(crate) const FINAL_LOOP_COMPLETE_EVENT: &str = "finalloopcomplete";

/// Which of an element's two sources a value is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageRole {
    /// The picture the element is for: `src`, or an `svg`'s `content`.
    Source,
    /// The picture it shows until [`ImageRole::Source`] has pixels.
    Placeholder,
}

/// One element's own source settling or playing, for the runtime to turn
/// into an event.
///
/// A placeholder produces none: that asymmetry is the type's, which has no
/// variant naming one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageOutcome {
    /// The element's source has loaded, as pixels or as a parsed vector
    /// document, with this intrinsic size.
    Loaded {
        node: NodeId,
        width: u32,
        height: u32,
    },
    /// The element's source will never produce pixels.
    Failed { node: NodeId },
    /// The element's animated source loaded and plays into `picture`: a
    /// `startplay` when that picture plays from the start, which the policy
    /// read at the end of the entry says ([`ImageSources::autoplays`]).
    StartPlay { node: NodeId, picture: PictureId },
    /// The element's animated source completed a loop
    /// (`currentloopcomplete`) or its final loop (`finalloopcomplete`): an
    /// event with an empty detail.
    Played { node: NodeId, name: &'static str },
}

impl ImageOutcome {
    /// The outcome `status` owes `node`'s own source, or `None` while it is
    /// pending.
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

/// What an element's outcomes for one write or one report are: a `load` or
/// an `error`, after a `startplay` when one is owed.
pub(crate) type ImageOutcomes = SmallVec<[ImageOutcome; 2]>;

/// The policy an `image`'s `loop-count` and `autoplay` give an animated
/// `src`, as native reads them: a `loop-count` that is absent, not a number,
/// zero or negative plays forever, and a positive one plays that many times,
/// a fraction truncated; the file's own count is not read. `autoplay` is on
/// unless it is `"false"`, the string `__SetAttribute` writes for `false`.
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

/// The sources one element names, and the picture it plays its `src` into.
#[derive(Debug, Default)]
struct Named {
    source: Option<Arc<str>>,
    placeholder: Option<Arc<str>>,
    /// An `image`: an animated `src` plays into a picture of its own.
    playable: bool,
    /// The picture an animated `src` plays into, while the element names it
    /// loaded.
    playing: Option<PictureId>,
}

/// A playback request queued for the host.
#[derive(Debug)]
enum Queued {
    /// `picture` starts playing `source` for `element`, under the policy the
    /// element's attributes give when the request is taken.
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
    /// What each element names. An element naming nothing has no entry.
    elements: FxHashMap<NodeId, Named>,
    /// The elements naming each source, in the order they named it, and in
    /// which role: a settling source owes its events in that order. One
    /// element naming one URL in both roles is two entries.
    holders: FxHashMap<Arc<str>, SmallVec<[(NodeId, ImageRole); 1]>>,
    /// The element each playing picture belongs to.
    players: FxHashMap<PictureId, NodeId>,
    /// The playback requests not handed to the host yet, in order.
    playback: Vec<Queued>,
    /// The playing pictures whose policy plays from the start.
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

    /// Ends the picture `element` plays its `src` into, if it plays one.
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

/// The sources of one document's `image` and `svg` elements.
///
/// A handle: the two components and the runtime's report path each hold a
/// clone of one state, as they each hold one [`super::ComponentEvents`].
/// Every method takes the document as an argument and borrows the state
/// only between calls into it.
#[derive(Clone, Debug, Default)]
pub(crate) struct ImageSources(Rc<RefCell<State>>);

impl ImageSources {
    /// Names `value` as `element`'s source in `role`, or none, and presents
    /// what the element now shows. Answers the outcome a `src` that had
    /// already settled owes at once.
    ///
    /// Writing the value a role already holds does nothing at all: it asks
    /// for nothing and owes nothing, since a picture that never moved owes
    /// no second `load`.
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

    /// [`Self::set`] for an `image`, whose animated `src` plays into a
    /// picture of its own: answers a `startplay` before the `load` when such
    /// a `src` had already settled and plays from the start.
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

    /// Names a document handed over as `bytes` — an `svg`'s `content` — as
    /// `element`'s source: [`Self::set`] with the synthetic source the
    /// document files the bytes under.
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

    /// What `settled` — the sources a batch of reports settled, as the
    /// document answered them — moves: every element naming one presents
    /// what it now shows, and each one whose own source it is owes an
    /// outcome, in report order and, within one source, in the order the
    /// elements named it; a `startplay` before the `load` of an animated
    /// `src` that plays from the start.
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

    /// The events the host's playback reports owe, in report order: a
    /// `currentloopcomplete` per loop and a `finalloopcomplete` after the
    /// last of a finite count, at the element whose picture played. A report
    /// for a picture that has ended since is dropped.
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
                        // A media player's, whose picture is a `video`'s
                        // ([`super::video`]), never one of these.
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

    /// Starts the playback of `element`'s `src`, which settled at `status`,
    /// when the element is an `image`, the source animates and nothing plays
    /// it yet: a picture of the element's own, whose request is queued for
    /// the end of the entry, when its policy is read
    /// ([`Self::take_playback`]). Answers the picture.
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

    /// Whether `picture` was played from the start: the `startplay` an
    /// [`ImageOutcome::StartPlay`] for it owes. Asked after the entry that
    /// queued the outcome has handed its requests to the host.
    pub(crate) fn autoplays(&self, picture: PictureId) -> bool {
        self.0.borrow().autoplaying.contains(&picture)
    }

    /// Sends `control` to the picture `element` plays its `src` into, if it
    /// plays one. Answers whether it does.
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

    /// Takes the playback requests made since the last call, for the host,
    /// in the order they were made. A picture that starts playing is played
    /// under the policy its element's `loop-count` and `autoplay` give now
    /// ([`play_policy`]).
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

    /// Drops everything kept for `element`, which is being freed, and ends
    /// the picture it played.
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

    /// The source `element` names in `role`.
    #[cfg(test)]
    pub(crate) fn source(&self, element: NodeId, role: ImageRole) -> Option<Arc<str>> {
        self.0
            .borrow()
            .elements
            .get(&element)
            .and_then(|named| named.get(role).cloned())
    }

    /// The picture `element` plays its `src` into.
    #[cfg(test)]
    pub(crate) fn playing(&self, element: NodeId) -> Option<PictureId> {
        self.0
            .borrow()
            .elements
            .get(&element)
            .and_then(|named| named.playing)
    }

    /// Puts on `element`'s box the picture it shows: its `src` while that is
    /// loaded — the picture it plays an animated `src` into, when it plays
    /// one — its `placeholder` while that is loaded and `src` is not,
    /// otherwise whichever it names; with that picture's natural size, or
    /// none while it has none. An element naming nothing presents nothing,
    /// and is not replaced.
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

/// Applies the painting side's image reports to `document`, and answers the
/// outcomes they owe the elements of `sources`: what the runtime queues for
/// its epilogue. The playback reports' events follow the settled sources'.
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
    // Every size asserted here is a reported pixel count, so no tolerance.
    #![allow(clippy::float_cmp)]

    use std::sync::Arc;

    use terminus::link::{ImageEvent, ImageStatus, NodeId, PictureKey};

    use super::super::test_support::{child, with_image_sources};
    use super::super::{LynxDocument, PageConfig};
    use super::{ImageOutcome, ImageRole, ImageSources, apply_image_events};

    // `SRC` is the picture the element is for; `PLACEHOLDER` is what it
    // draws until `SRC` has pixels.
    const SRC: &str = "app:///a.png";
    const OTHER_SRC: &str = "app:///b.png";
    const PLACEHOLDER: &str = "app:///p.png";

    fn images() -> (LynxDocument, ImageSources) {
        let (document, _, sources) = with_image_sources(PageConfig::default());
        (document, sources)
    }

    /// An element the record's sources are set on directly: absolutely
    /// positioned and unsized, so its box is its natural size once replaced.
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

    /// The URL `element` presents.
    fn presented(document: &LynxDocument, element: NodeId) -> Option<String> {
        document
            .picture(element)
            .and_then(PictureKey::url)
            .map(str::to_owned)
    }

    /// The size `element`'s box lays out at: its natural size, being
    /// unsized, or nothing while it has none.
    fn natural(document: &mut LynxDocument, element: NodeId) -> (f32, f32) {
        document.layout();
        let layout = document.rounded_layout(element).expect("laid out");
        (layout.size.width, layout.size.height)
    }

    /// The natural size is the drawn bitmap's, whichever source that is —
    /// `object-fit` fits one against the other at paint.
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

    /// And the suppression is permanent: a placeholder whose load lands after
    /// the source's changes nothing visible.
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

    /// A failed source is not a failed element: the placeholder it was
    /// covering stays drawn, and one that loads afterwards still appears.
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

    /// A placeholder is a bitmap in the content box like any other, so an
    /// element that has only one is replaced content and sizes from it.
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

    /// Swapping one source for another blanks the element until the new one
    /// loads, rather than fitting the new bitmap to the departed one's size.
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

        // With a placeholder ready, that is what the blanked element shows.
        sources.set(
            &mut document,
            image,
            ImageRole::Placeholder,
            Some(PLACEHOLDER),
        );
        apply_image_events(&mut document, &sources, &[loaded(PLACEHOLDER, 4, 4)]);
        assert_eq!(natural(&mut document, image), (4.0, 4.0));
    }

    /// A source that settled before this element named it is reported by
    /// nothing else ever again — one URL is reported once — so the write is
    /// where its outcome and its size both arrive.
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

    /// Rewriting the source that is already there changes nothing, and
    /// reports nothing — a second `load` for a picture that never moved.
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

    /// Only the elements the source belongs to are reported: a placeholder is
    /// an interim picture the page did not ask about.
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

        // Nor at the write, which is the other place an outcome is produced:
        // the role decides, so a settled URL named as a placeholder answers
        // with nothing.
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

    /// A failure names its elements: an `error` is owed to exactly the
    /// elements the source belongs to, once.
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

    /// A zero axis is a failure, and is owed as one rather than as the load
    /// the host wrote.
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

    /// One element may name one URL in both roles, so a holder is the pair
    /// and not the element: clearing one role must leave the other standing,
    /// and only the source role owes an outcome.
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

    /// Taking the last source off an element makes it an ordinary one again,
    /// natural size and all: there is no bitmap left for the box to be
    /// sized from or to hide its children for.
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

    /// Regression: a load completing after its element was freed must not
    /// reach the freed id. A free drops the element's record, both of its
    /// sources included, so the report owes it nothing and sizes nothing.
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

    /// And the record is per element, not per source: one element going
    /// away leaves the others naming the same URL, so the load still sizes
    /// them and still owes them their `load`.
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
