//! The events this crate's own elements queue for the embedder to fire.

use std::rc::Rc;

use crate::tree::document::{Document, NodeId};

pub trait QueuedEventSink {
    fn queue(&self, target: NodeId, name: &'static str);
}

impl<T> Document<T> {
    pub fn set_queued_event_sink(&mut self, sink: Rc<dyn QueuedEventSink>) {
        self.queued_events = Some(sink);
    }

    pub(crate) fn queue_element_event(&self, target: NodeId, name: &'static str) {
        if let Some(sink) = &self.queued_events {
            sink.queue(target, name);
        }
    }
}
