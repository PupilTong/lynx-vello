//! This dedicated test binary gets a fresh process, so the barrier below
//! races the first initialization of the process-global class IDs: the host
//! function owner, the `ShadowRealm` instance and the realm link, which one
//! initializer allocates together.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Barrier};
use std::thread;

use quickjs_rust_bridge::{EvalOptions, EvalSource, HostValue, Runtime};

#[test]
fn concurrent_realms_share_safely_allocated_class_ids() {
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
                    let children = Rc::new(RefCell::new(Vec::new()));
                    let kept = Rc::clone(&children);
                    realm
                        .install_shadow_realm(move |child| {
                            kept.borrow_mut().push(child);
                            Ok(())
                        })
                        .unwrap();
                    assert_eq!(
                        realm
                            .evaluate(
                                EvalSource::new(
                                    "new ShadowRealm().evaluate('(f) => f() / 2')(answer) * 2"
                                ),
                                EvalOptions::default()
                            )
                            .unwrap()
                            .as_number(),
                        Some(42.0)
                    );
                    assert_eq!(children.borrow().len(), 1);
                }
            })
        })
        .collect();

    for worker in workers {
        worker.join().unwrap();
    }
}
