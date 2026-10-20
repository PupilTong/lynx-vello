//! How many fingers are down on each element of a component: the count a
//! component that reports `isDragging` or `isDragged` reads.
//!
//! The count is the component's own reading of the touch events that reach
//! its element — `touchstart` adds one, `touchend` and `touchcancel` take
//! one away, never below zero — and nothing the painter reports: a finger
//! down on the element, or on anything inside it, counts whether or not it
//! drags anything. That is web-core's approximation of the native drag
//! state (its components set a flag on `touchstart` and clear it on
//! `touchend`), and the user's ruling of 2026-10-20 for this engine.
//!
//! A touch event bubbles and is composed, and the painter dispatches it at
//! the node the finger's down routed to for the whole touch (implicit
//! capture), so a `touchend` reaches every element its `touchstart` did. A
//! component's element on that path is called once per pass, so each event
//! is counted at one delivery only: the bubble pass for an ancestor of the
//! target, and the one at-target delivery for the element itself or for a
//! host standing in for a node of its shadow tree, which has no bubble-pass
//! step ([`terminus::event::ElementEvent`]'s walk calls a target once). The
//! capture-pass delivery of the same event is never counted.
//!
//! An element with no finger down holds no entry, so the table is as long
//! as the number of elements touched right now.

use std::cell::RefCell;

use smallvec::SmallVec;
use terminus::NodeId;
use terminus::event::{ElementEvent, EventPhase};

/// The touch event a finger going down dispatches.
pub(super) const TOUCH_START: &str = "touchstart";
/// The touch event a finger lifting dispatches.
pub(super) const TOUCH_END: &str = "touchend";
/// The touch event a touch the platform took back dispatches.
pub(super) const TOUCH_CANCEL: &str = "touchcancel";

/// The fingers down per element: a component's own table, which it feeds
/// every touch event its element hears ([`Self::observe`]), asks
/// ([`Self::is_down`]) and empties for an element that leaves the document
/// or is freed ([`Self::forget`]). A component that keeps one names
/// [`TOUCH_START`], [`TOUCH_END`] and [`TOUCH_CANCEL`] in its
/// `observed_events`.
#[derive(Debug, Default)]
pub(super) struct TouchCount(RefCell<SmallVec<[(NodeId, u32); 2]>>);

impl TouchCount {
    /// Counts `event` at `element`, the element whose handler is running.
    ///
    /// Answers whether this delivery is the one the event counts at — a
    /// touch event, outside the capture pass — so a component that acts on a
    /// `touchend` acts on the same delivery, once. Any other event, and the
    /// capture-pass delivery of a touch, change nothing and answer `false`.
    pub(super) fn observe(&self, element: NodeId, event: &ElementEvent<'_>) -> bool {
        let name = event.type_name();
        if event.phase() == EventPhase::Capturing
            || !matches!(name, TOUCH_START | TOUCH_END | TOUCH_CANCEL)
        {
            return false;
        }
        let mut table = self.0.borrow_mut();
        let entry = table.iter().position(|&(held, _)| held == element);
        match (name, entry) {
            (TOUCH_START, Some(index)) => table[index].1 = table[index].1.saturating_add(1),
            (TOUCH_START, None) => table.push((element, 1)),
            (_, Some(index)) => {
                table[index].1 -= 1;
                if table[index].1 == 0 {
                    table.swap_remove(index);
                }
            }
            // A lift with no finger counted down: one that went down before
            // the element was connected, or before the count was forgotten.
            (_, None) => {}
        }
        true
    }

    /// Whether at least one finger is down on `element` or inside it.
    pub(super) fn is_down(&self, element: NodeId) -> bool {
        self.0.borrow().iter().any(|&(held, _)| held == element)
    }

    /// Drops `element`'s count: it left the document, where no finger can
    /// lift on it, or was freed.
    pub(super) fn forget(&self, element: NodeId) {
        self.0.borrow_mut().retain(|(held, _)| *held != element);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use terminus::event::{ElementEvent, EventDetail};
    use terminus::{CustomElement, NodeId};

    use super::super::LynxDocument;
    use super::super::test_support::{child, document, element_under};
    use super::{TOUCH_CANCEL, TOUCH_END, TOUCH_START, TouchCount};

    const TAG: &str = "x-touch-probe";

    /// A test component whose only state is a [`TouchCount`] table, which it
    /// shares with the test, and which records each delivery it counted.
    struct Probe {
        touch_count: Rc<TouchCount>,
        counted: Rc<RefCell<Vec<&'static str>>>,
    }

    impl CustomElement<()> for Probe {
        fn observed_events(&self) -> &'static [&'static str] {
            &[TOUCH_START, TOUCH_END, TOUCH_CANCEL, "tap"]
        }

        fn disconnected_callback(&self, _document: &LynxDocument, element: NodeId) {
            self.touch_count.forget(element);
        }

        fn freed(&self, element: NodeId) {
            self.touch_count.forget(element);
        }

        fn handle_event(
            &self,
            _document: &mut LynxDocument,
            element: NodeId,
            event: &mut ElementEvent<'_>,
        ) {
            if self.touch_count.observe(element, event) {
                self.counted.borrow_mut().push(event.type_name());
            }
        }
    }

    struct Fixture {
        document: LynxDocument,
        touch_count: Rc<TouchCount>,
        counted: Rc<RefCell<Vec<&'static str>>>,
        host: NodeId,
        inner: NodeId,
    }

    /// A probe under the page, with a `view` inside it.
    fn fixture() -> Fixture {
        let mut document = document();
        let touches = Rc::new(TouchCount::default());
        let counted = Rc::new(RefCell::new(Vec::new()));
        document.define(
            TAG,
            Box::new(Probe {
                touch_count: Rc::clone(&touches),
                counted: Rc::clone(&counted),
            }),
        );
        let host = child(&mut document, TAG, "");
        let inner = element_under(&mut document, host, "view", "");
        Fixture {
            document,
            touch_count: touches,
            counted,
            host,
            inner,
        }
    }

    /// Dispatches `name` at `target` as the runtime dispatches a routed
    /// touch event: bubbling and composed.
    fn touch(document: &mut LynxDocument, target: NodeId, name: &'static str) {
        document.dispatch_element_event(target, name, &EventDetail::None, true, true, 0.0);
    }

    #[test]
    fn a_touch_inside_the_element_counts_once_per_event() {
        let Fixture {
            mut document,
            touch_count: touches,
            counted,
            host,
            inner,
        } = fixture();
        assert!(!touches.is_down(host));
        touch(&mut document, inner, TOUCH_START);
        assert!(touches.is_down(host));
        assert_eq!(
            *counted.borrow(),
            [TOUCH_START],
            "the capture-pass delivery is not counted, the bubble-pass one is"
        );
        touch(&mut document, inner, TOUCH_END);
        assert!(!touches.is_down(host));
        assert_eq!(*counted.borrow(), [TOUCH_START, TOUCH_END]);
    }

    #[test]
    fn a_touch_at_the_element_itself_counts_at_its_one_delivery() {
        let Fixture {
            mut document,
            touch_count: touches,
            counted,
            host,
            ..
        } = fixture();
        touch(&mut document, host, TOUCH_START);
        assert!(touches.is_down(host), "the at-target delivery counts");
        touch(&mut document, host, TOUCH_CANCEL);
        assert!(!touches.is_down(host), "a cancel lifts the finger too");
        assert_eq!(*counted.borrow(), [TOUCH_START, TOUCH_CANCEL]);
    }

    #[test]
    fn two_fingers_are_down_until_both_lift_and_a_stray_lift_saturates() {
        let Fixture {
            mut document,
            touch_count: touches,
            host,
            inner,
            ..
        } = fixture();
        touch(&mut document, host, TOUCH_END);
        assert!(
            !touches.is_down(host),
            "a lift with nothing down stays at zero"
        );
        touch(&mut document, inner, TOUCH_START);
        touch(&mut document, host, TOUCH_START);
        touch(&mut document, inner, TOUCH_END);
        assert!(touches.is_down(host), "one finger is still down");
        touch(&mut document, host, TOUCH_END);
        assert!(!touches.is_down(host));
        touch(&mut document, host, TOUCH_START);
        assert!(touches.is_down(host), "the count starts again from zero");
    }

    #[test]
    fn another_event_counts_nothing() {
        let Fixture {
            mut document,
            touch_count: touches,
            counted,
            host,
            inner,
        } = fixture();
        touch(&mut document, inner, "tap");
        assert!(!touches.is_down(host));
        assert!(counted.borrow().is_empty());
    }

    #[test]
    fn leaving_the_document_or_being_freed_forgets_the_count() {
        let Fixture {
            mut document,
            touch_count: touches,
            host,
            inner,
            ..
        } = fixture();
        let page = document.document_element().id();
        touch(&mut document, inner, TOUCH_START);
        document.remove_element(host);
        assert!(!touches.is_down(host), "a disconnection forgets");
        document.append_child(page, host);
        touch(&mut document, inner, TOUCH_END);
        assert!(!touches.is_down(host), "the lift of a forgotten finger");

        // A detached element still hears a touch dispatched inside it, and
        // its free, which no disconnection precedes, forgets it.
        document.remove_element(host);
        touch(&mut document, inner, TOUCH_START);
        assert!(touches.is_down(host));
        document.drop_subtree(host);
        assert!(!touches.is_down(host), "a free forgets");
        assert!(touches.0.borrow().is_empty(), "no entry is left behind");
    }
}
