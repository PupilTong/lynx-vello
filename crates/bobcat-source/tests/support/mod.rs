//! Hold a real BTS entry while the host continues serving MTS and frames.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use bobcat_core::resource::{ResourceFetcher, SourceCompletion, SourceRequest};
use bobcat_resources::ViewResources;

pub type PendingSource = Rc<RefCell<Option<(SourceRequest, SourceCompletion)>>>;

pub struct DelayedBackground {
    pub resources: ViewResources,
    pub background: String,
    pub pending: PendingSource,
    pub released: Rc<Cell<bool>>,
}

impl bobcat_core::FrameImages for DelayedBackground {
    fn read(
        &self,
        source: &str,
        hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::vello::peniko::ImageData> {
        self.resources.read(source, hint)
    }

    fn retain(&self, frame: &[Arc<str>]) {
        self.resources.retain(frame);
    }
}

impl ResourceFetcher for DelayedBackground {
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion) {
        if matches!(&request, SourceRequest::Module(url) if url == &self.background)
            && !self.released.get()
        {
            assert!(self.pending.borrow().is_none());
            *self.pending.borrow_mut() = Some((request, completion));
            return;
        }
        self.resources.request_source(request, completion);
    }

    fn service_images(&self) {
        if self.released.get()
            && let Some((request, completion)) = self.pending.borrow_mut().take()
        {
            self.resources.request_source(request, completion);
        }
        self.resources.service_images();
    }
}
