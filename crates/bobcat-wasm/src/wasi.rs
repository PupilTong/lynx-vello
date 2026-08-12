//! NAPI-RS surface compiled for the browser's threaded WASI runtime.

use std::sync::atomic::{AtomicU32, Ordering};

use napi::bindgen_prelude::{AsyncTask, Uint8Array};
use napi::{Env, Task};
use napi_derive::napi;

const MAX_THREADS: u32 = 8;

/// Result of work that ran on real Rust threads in the WASI module.
#[napi(object)]
#[derive(Debug)]
pub struct ThreadReport {
    pub checksum: u32,
    pub threads: u32,
}

#[derive(Debug)]
pub struct ChecksumTask {
    bytes: Vec<u8>,
    threads: u32,
}

#[napi]
impl Task for ChecksumTask {
    type Output = ThreadReport;
    type JsValue = ThreadReport;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        let threads = self.threads.clamp(1, MAX_THREADS);
        let checksum = AtomicU32::new(0);
        let chunk_size = self.bytes.len().div_ceil(threads as usize).max(1);

        std::thread::scope(|scope| -> napi::Result<()> {
            let mut workers = Vec::with_capacity(threads as usize);
            for index in 0..threads as usize {
                let start = index.saturating_mul(chunk_size);
                let end = (start + chunk_size).min(self.bytes.len());
                let bytes = self.bytes.get(start..end).unwrap_or_default();
                let checksum = &checksum;
                let worker = std::thread::Builder::new()
                    .name(format!("bobcat-checksum-{index}"))
                    .spawn_scoped(scope, move || {
                        let local = bytes
                            .iter()
                            .fold(0_u32, |sum, byte| sum.wrapping_add(u32::from(*byte)));
                        checksum.fetch_add(local, Ordering::Relaxed);
                    });

                match worker {
                    Ok(worker) => workers.push(worker),
                    Err(error) => {
                        join_workers(workers)?;
                        return Err(napi::Error::from_reason(format!(
                            "could not start checksum thread {index}: {error}"
                        )));
                    }
                }
            }

            join_workers(workers)
        })?;

        Ok(ThreadReport {
            checksum: checksum.load(Ordering::Relaxed),
            threads,
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        Ok(output)
    }
}

fn join_workers(workers: Vec<std::thread::ScopedJoinHandle<'_, ()>>) -> napi::Result<()> {
    for worker in workers {
        worker
            .join()
            .map_err(|_| napi::Error::from_reason("a checksum thread panicked"))?;
    }
    Ok(())
}

/// Runs a wrapping byte checksum on `threads` scoped Rust threads.
///
/// NAPI-RS executes [`Task::compute`] in its async-work pool, so the blocking
/// joins never execute on the browser UI thread.
#[napi]
#[must_use]
pub fn parallel_checksum(bytes: Uint8Array, threads: u32) -> AsyncTask<ChecksumTask> {
    let owned_bytes = bytes.to_vec();
    // Release the N-API reference on the calling JavaScript thread before the
    // detached task starts; only the copied Rust buffer crosses the boundary.
    drop(bytes);
    AsyncTask::new(ChecksumTask {
        bytes: owned_bytes,
        threads,
    })
}
