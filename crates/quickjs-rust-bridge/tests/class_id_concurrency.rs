//! This dedicated test binary gets a fresh process, so the barrier below
//! races the first initialization of the process-global host class ID.

use std::sync::{Arc, Barrier};
use std::thread;

use quickjs_rust_bridge::{EvalOptions, EvalSource, HostValue, Runtime};

#[test]
fn concurrent_realms_share_one_safely_allocated_host_class_id() {
    const WORKERS: usize = 8;

    let barrier = Arc::new(Barrier::new(WORKERS));
    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..100 {
                    let mut realm = Runtime::new().unwrap().create_context().unwrap();
                    realm
                        .define_global_function("answer", 0, |_| Ok(HostValue::Number(42.0)))
                        .unwrap();
                    assert_eq!(
                        realm
                            .evaluate(EvalSource::new("answer()"), EvalOptions::default())
                            .unwrap()
                            .as_number(),
                        Some(42.0)
                    );
                }
            })
        })
        .collect();

    for worker in workers {
        worker.join().unwrap();
    }
}
