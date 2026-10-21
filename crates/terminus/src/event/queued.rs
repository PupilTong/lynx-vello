//! The events this crate's own elements queue for the embedder to fire.

use std::rc::Rc;

use crate::tree::document::{Document, NodeId};

/// Where the events that this crate's own elements ([`crate::html`]) queue
/// go: HTML's "queue an element task … to fire an event named *name* at
/// *element*", for an embedder that fires them later, outside the call that
/// queued them.
///
/// The crate dispatches no script event itself. An element implemented here
/// hands each event to the document's sink at the moment the standard queues
/// it, so an embedder that queues its own components' events in the same
/// place keeps all of them in the order they formed. `name` is the event's
/// type; what the event carries and whether it bubbles are the element's
/// documentation's.
///
/// Registered once per document with [`Document::set_queued_event_sink`].
/// With none registered, a queued event is dropped.
pub trait QueuedEventSink {
    /// Queues the event `name` at `target`. Called inside a document
    /// mutation, with the document borrowed, so it can only record the
    /// event.
    fn queue(&self, target: NodeId, name: &'static str);
}

impl<T> Document<T> {
    /// Registers the sink every event this document's elements queue goes
    /// to ([`QueuedEventSink`]), replacing any earlier one. Until one is
    /// registered, those events are dropped.
    pub fn set_queued_event_sink(&mut self, sink: Rc<dyn QueuedEventSink>) {
        self.queued_events = Some(sink);
    }

    /// Hands the event `name` at `target` to the registered sink, or drops
    /// it when there is none.
    pub(crate) fn queue_element_event(&self, target: NodeId, name: &'static str) {
        if let Some(sink) = &self.queued_events {
            sink.queue(target, name);
        }
    }
}
