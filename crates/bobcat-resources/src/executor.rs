//! Where the fetcher's own work runs, and how it gets back.
//!
//! The protocol initiates loads on the painter's thread and promises no
//! ambient runtime, so natively this crate brings one of its own: a
//! `current_thread` tokio runtime that [`Executor::new`] builds on the thread
//! constructing the [`Resources`](crate::Resources) and hands to a thread
//! named `bobcat-resources-driver`, which drives it and shuts it down. Its
//! blocking pool runs every transport read, preprocessing pass, platform
//! decode and SVG document parse. Browser IO has no runtime here at all — it
//! runs as local futures on the Render Worker's event loop.
//!
//! Images wake the host to service reports; sources complete directly into
//! main's FIFO, whose lifecycle notifications subsequently wake the host. An
//! animated image's timer wakes the host the same way when its next frame is
//! due: natively a task sleeping on this runtime's timer, in the browser a
//! local task awaiting the Render Worker's `setTimeout` (`sleep`).

use std::sync::Arc;

pub type Wakeup = Arc<dyn Fn() + Send + Sync>;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Executor {
    handle: tokio::runtime::Handle,
    stop: tokio_util::sync::CancellationToken,
    driver: Option<std::thread::JoinHandle<()>>,
    decode_permits: Arc<tokio::sync::Semaphore>,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Debug for Executor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Executor")
            .field("decode_permits", &self.decode_permits.available_permits())
            .finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Executor {
    pub(crate) fn new(worker_threads: usize, decode_parallelism: Option<usize>) -> Self {
        let threads = worker_threads.max(1);
        let permits = decode_parallelism.unwrap_or(threads).max(1);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .thread_name("bobcat-resources")
            .max_blocking_threads(threads)
            .enable_time()
            .build()
            .expect("the bobcat-resources runtime must initialize");
        let handle = runtime.handle().clone();
        let stop = tokio_util::sync::CancellationToken::new();
        let driver = std::thread::Builder::new()
            .name("bobcat-resources-driver".to_owned())
            .spawn({
                let stop = stop.clone();
                move || {
                    runtime.block_on(stop.cancelled());
                    runtime.shutdown_background();
                }
            })
            .expect("the bobcat-resources driver thread must start");
        Self {
            handle,
            stop,
            driver: Some(driver),
            decode_permits: Arc::new(tokio::sync::Semaphore::new(permits)),
        }
    }

    pub(crate) fn spawn<F>(&self, job: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        drop(self.handle.spawn(job));
    }

    pub(crate) fn handle(&self) -> tokio::runtime::Handle {
        self.handle.clone()
    }

    pub(crate) fn decode_permits(&self) -> Arc<tokio::sync::Semaphore> {
        Arc::clone(&self.decode_permits)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn blocking<T: Send + 'static>(
    handle: &tokio::runtime::Handle,
    what: &'static str,
    job: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    match handle.spawn_blocking(job).await {
        Ok(value) => Ok(value),
        Err(error) if error.is_panic() => Err(format!(
            "the {what} panicked: {}",
            panic_message(&*error.into_panic())
        )),
        Err(_) => Err(format!(
            "the {what} was dropped when the resource system shut down"
        )),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&'static str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".to_owned())
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Executor {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(driver) = self.driver.take() {
            let _ = driver.join();
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) async fn sleep(duration: std::time::Duration) {
    use js_sys::wasm_bindgen::{JsCast, JsValue};

    let millis = i32::try_from(duration.as_millis().saturating_add(1)).unwrap_or(i32::MAX);
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let global = js_sys::global();
        let set_timeout = js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout"))
            .ok()
            .and_then(|value| value.dyn_into::<js_sys::Function>().ok());
        if let Some(set_timeout) = set_timeout {
            let _ = set_timeout.call2(&global, &resolve, &JsValue::from(millis));
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}
