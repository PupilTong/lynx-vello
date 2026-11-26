//! Structured clones written with a transfer list: a host function whose
//! argument is a message and whose next argument is its transfer list, and
//! the delivery of such a message into another realm with one replacement
//! per transfer slot.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;

use quickjs_rust_bridge::{
    CallOutcome, Context, ErrorKind, EvalOptions, EvalSource, HostArgument, HostValue, Revival,
    Runtime, RuntimeOptions, SourceType, TransferredClone, Value,
};

const HOST: &str = "test:host";

struct Party {
    runtime: Runtime,
    realm: Context,
    host: Value,
    posted: Rc<RefCell<Vec<Vec<HostValue>>>>,
}

fn realm_with(options: RuntimeOptions, message_index: u32) -> Party {
    let mut runtime = Runtime::with_options(options).unwrap();
    let realm = runtime.create_context().unwrap();
    let posted = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&posted);
    realm
        .register_host_module_function_with_transfer(
            HOST,
            "post",
            message_index + 2,
            message_index,
            move |arguments| {
                sink.borrow_mut().push(
                    arguments
                        .iter()
                        .map(|argument| argument.to_value())
                        .collect(),
                );
                Ok(HostValue::Undefined)
            },
        )
        .unwrap();
    realm
        .register_host_module_transfer_members(HOST, "mark", "setReviver")
        .unwrap();
    let evaluation = realm
        .evaluate(
            EvalSource::new(
                "import { post, mark, setReviver } from 'test:host';\n\
                 Object.assign(globalThis, { post, mark, setReviver });\n\
                 globalThis.host = { seen: null, take(value) { host.seen = value; } };",
            ),
            EvalOptions {
                source_type: SourceType::Module,
                ..EvalOptions::default()
            },
        )
        .unwrap();
    runtime.drain_pending_jobs(&realm).unwrap();
    assert!(realm.settled_promise_result(&evaluation).unwrap().is_some());
    let host = realm
        .evaluate(EvalSource::new("globalThis.host"), EvalOptions::default())
        .unwrap();
    Party {
        runtime,
        realm,
        host,
        posted,
    }
}

fn realm() -> Party {
    realm_with(RuntimeOptions::default(), 0)
}

impl Party {
    fn text(&self, source: &str) -> String {
        let value = self
            .realm
            .evaluate(EvalSource::new(source), EvalOptions::default())
            .unwrap();
        String::from_utf16(&value.to_utf16().unwrap()).unwrap()
    }

    fn take_posted(&self) -> Vec<HostValue> {
        let mut posted = self.posted.borrow_mut();
        assert_eq!(posted.len(), 1, "one post: {posted:?}");
        posted.pop().unwrap()
    }

    fn deliver(&self, argument: HostArgument<'_>) -> Result<(), quickjs_rust_bridge::Error> {
        let take = self.realm.member("take").unwrap();
        let outcome = self.realm.call_member(&self.host, &take, &[argument])?;
        assert!(matches!(outcome, CallOutcome::Called(_)));
        Ok(())
    }
}

fn transferred(posted: &[HostValue]) -> &TransferredClone {
    match &posted[0] {
        HostValue::Transferred(clone) => clone,
        other => panic!("not a transferred clone: {other:?}"),
    }
}

#[test]
fn a_listed_object_reads_as_one_replacement_at_every_position() {
    let sender = realm();
    sender.text(
        "(() => {
            const canvas = { id: 7 };
            mark(canvas, true);
            const shared = { x: 1 };
            const message = { a: canvas, nest: { b: canvas }, list: [canvas, 1, canvas],
                              s1: shared, s2: shared };
            message.self = message;
            post(message, [canvas], 7);
            return '';
        })()",
    );
    let posted = sender.take_posted();
    let clone = transferred(&posted);
    assert_eq!(clone.slots(), 1);
    assert_eq!(posted[1], HostValue::Number(1.0), "the list is its length");
    assert_eq!(
        posted[2],
        HostValue::Number(7.0),
        "the ids follow as primitives"
    );

    let receiver = realm();
    receiver.text(
        "globalThis.revived = [];
         setReviver((descriptor) => { const r = { revived: descriptor }; revived.push(r); return r; }); ''",
    );
    let revival = Revival::new(clone.as_bytes(), &[HostArgument::Number(70.0)]).unwrap();
    receiver.deliver(HostArgument::Revive(&revival)).unwrap();
    assert_eq!(
        receiver.text(
            "(() => {
                const m = host.seen;
                return [
                    revived.length, m.a.revived, m.a === revived[0], m.nest.b === m.a,
                    m.list[0] === m.a, m.list[2] === m.a, m.list[1],
                    m.self === m, m.s1 === m.s2, m.s1 !== m.a, m.s1.x,
                ].join(':');
            })()",
        ),
        "1:70:true:true:true:true:1:true:true:true:1"
    );
}

#[test]
fn every_listed_object_takes_a_slot_in_list_order_whether_or_not_the_message_holds_it() {
    let sender = realm();
    sender.text(
        "(() => {
            const a = {}, b = {}, c = {};
            for (const o of [a, b, c]) mark(o, true);
            post({ second: b, first: a }, [a, b, c], 1, 2, 3);
            return '';
        })()",
    );
    let posted = sender.take_posted();
    let clone = transferred(&posted);
    assert_eq!(clone.slots(), 3);
    assert_eq!(posted[1], HostValue::Number(3.0));

    let receiver = realm();
    receiver.text("globalThis.order = []; setReviver((d) => { order.push(d); return { d }; }); ''");
    let descriptors = [
        HostArgument::String("first"),
        HostArgument::String("second"),
        HostArgument::String("third"),
    ];
    let revival = Revival::new(clone.as_bytes(), &descriptors).unwrap();
    receiver.deliver(HostArgument::Revive(&revival)).unwrap();
    assert_eq!(
        receiver.text("[order.join(','), host.seen.first.d, host.seen.second.d].join(':')"),
        "first,second,third:first:second"
    );
}

#[test]
fn a_primitive_message_with_a_list_still_has_its_slots() {
    let sender = realm();
    sender.text("(() => { const c = {}; mark(c, true); post(5, [c]); return ''; })()");
    let posted = sender.take_posted();
    let clone = transferred(&posted);
    assert_eq!(clone.slots(), 1);

    let receiver = realm();
    receiver.text("globalThis.calls = 0; setReviver(() => { calls++; return {}; }); ''");
    let revival = Revival::new(clone.as_bytes(), &[HostArgument::Undefined]).unwrap();
    receiver.deliver(HostArgument::Revive(&revival)).unwrap();
    assert_eq!(receiver.text("[host.seen, calls].join(':')"), "5:1");
}

#[test]
fn the_message_may_follow_other_arguments() {
    let sender = realm_with(RuntimeOptions::default(), 1);
    sender.text("(() => { const c = {}; mark(c, true); post(42, { c }, [c], 9); return ''; })()");
    let posted = sender.take_posted();
    assert_eq!(posted[0], HostValue::Number(42.0));
    assert_eq!(transferred(&posted[1..]).slots(), 1);
    assert_eq!(posted[2], HostValue::Number(1.0));
    assert_eq!(posted[3], HostValue::Number(9.0));
}

#[test]
fn an_empty_or_absent_list_describes_the_message_as_any_argument() {
    let sender = realm();
    sender
        .text("post({ a: 1 }); post({ a: 1 }, []); post(new ArrayBuffer(2), []); post('text'); ''");
    let posted = sender.posted.borrow();
    assert_eq!(posted.len(), 4);
    assert!(matches!(posted[0][0], HostValue::Structured(_)));
    assert_eq!(posted[0][1], HostValue::Number(0.0));
    assert!(matches!(posted[1][0], HostValue::Structured(_)));
    assert_eq!(posted[1][1], HostValue::Number(0.0));
    assert!(matches!(&posted[2][0], HostValue::Bytes(bytes) if bytes.len() == 2));
    assert_eq!(posted[3][0], HostValue::String("text".to_owned()));

    let receiver = realm();
    receiver.deliver(posted[0][0].as_argument()).unwrap();
    assert_eq!(receiver.text("String(host.seen.a)"), "1");
}

#[test]
fn a_marked_object_that_is_not_listed_throws_and_nothing_is_sent() {
    let mut sender = realm();
    let echoed = Rc::new(Cell::new(0u32));
    let counter = Rc::clone(&echoed);
    sender
        .realm
        .define_global_function("echo", 1, move |arguments| {
            counter.set(counter.get() + 1);
            Ok(arguments[0].to_value())
        })
        .unwrap();
    assert_eq!(
        sender.text(
            "(() => {
                const canvas = {}, other = {};
                mark(canvas, true);
                mark(other, true);
                const outcomes = [];
                for (const send of [
                    () => post(canvas),
                    () => post({ deep: [canvas] }, []),
                    () => post({ deep: [canvas], other }, [other]),
                    () => echo({ canvas }),
                    () => echo(canvas),
                ]) {
                    try { send(); outcomes.push('sent'); }
                    catch (e) { outcomes.push(`${e.name}: ${e.message}`); }
                }
                return outcomes.join('|');
            })()",
        ),
        [
            "TypeError: uncloneable object",
            "TypeError: uncloneable object",
            "TypeError: uncloneable object",
            "TypeError: uncloneable object",
            "TypeError: uncloneable object",
        ]
        .join("|")
    );
    assert!(sender.posted.borrow().is_empty());
    assert_eq!(echoed.get(), 0);
    sender.text("post({ fine: true }); ''");
    assert_eq!(sender.posted.borrow().len(), 1);
}

#[test]
fn a_list_entry_that_cannot_be_transferred_throws_and_nothing_is_sent() {
    let sender = realm();
    assert_eq!(
        sender.text(
            "(() => {
                const plain = {}, untransferable = {}, detached = {}, canvas = {};
                mark(untransferable, false);
                mark(detached, true);
                mark(detached, false);
                mark(canvas, true);
                const outcomes = [];
                for (const [message, list] of [
                    [{ plain }, [plain]],
                    [{}, [1]],
                    [{}, [untransferable]],
                    [{}, [detached]],
                    [{ canvas }, [canvas, canvas]],
                    [{ canvas }, { 0: canvas, length: 1 }],
                    [{ canvas }, null],
                ]) {
                    try { post(message, list); outcomes.push('sent'); }
                    catch (e) { outcomes.push(`${e.name}: ${e.message}`); }
                }
                return outcomes.join('|');
            })()",
        ),
        [
            "TypeError: value is not transferable",
            "TypeError: value is not transferable",
            "TypeError: value is not transferable",
            "TypeError: value is not transferable",
            "TypeError: duplicate transferable",
            "TypeError: the transfer list is not an array",
            "TypeError: the transfer list is not an array",
        ]
        .join("|")
    );
    assert!(sender.posted.borrow().is_empty());
    assert_eq!(
        sender.text("(() => { try { mark(1, true); } catch (e) { return e.name; } })()"),
        "TypeError"
    );
}

#[test]
fn a_stream_with_slots_does_not_read_without_its_replacements() {
    let sender = realm();
    sender.text("(() => { const a = {}, b = {}; mark(a, true); mark(b, true); post({ a, b }, [a, b]); return ''; })()");
    let posted = sender.take_posted();
    let clone = transferred(&posted);

    let error = Revival::new(clone.as_bytes(), &[HostArgument::Null])
        .expect_err("one descriptor for two slots");
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    let nested = Revival::new(
        clone.as_bytes(),
        &[
            HostArgument::Null,
            HostArgument::Transferred(clone.as_bytes()),
        ],
    )
    .expect_err("a transferred descriptor");
    assert_eq!(nested.kind, ErrorKind::InvalidInput);

    let receiver = realm();
    receiver.text("setReviver(() => ({})); ''");
    let error = receiver
        .deliver(posted[0].as_argument())
        .expect_err("no replacements");
    assert_eq!(error.name.as_deref(), Some("SyntaxError"));
    assert!(error.message.contains("invalid transfer slot"), "{error:?}");
    assert_eq!(receiver.text("String(host.seen)"), "null");
    receiver
        .deliver(HostArgument::Revive(
            &Revival::new(clone.as_bytes(), &[HostArgument::Null, HostArgument::Null]).unwrap(),
        ))
        .unwrap();
    assert_eq!(receiver.text("typeof host.seen.a"), "object");
}

#[test]
fn a_missing_or_throwing_reviver_fails_the_delivery_before_the_read() {
    let sender = realm();
    sender.text("(() => { const a = {}; mark(a, true); post({ a }, [a]); return ''; })()");
    let posted = sender.take_posted();
    let clone = transferred(&posted);
    let revival = Revival::new(clone.as_bytes(), &[HostArgument::Number(1.0)]).unwrap();

    let receiver = realm();
    let error = receiver
        .deliver(HostArgument::Revive(&revival))
        .expect_err("no reviver");
    assert_eq!(
        (error.name.as_deref(), error.message.as_str()),
        (Some("TypeError"), "this realm has no transfer reviver")
    );
    receiver.text("setReviver(() => { throw new RangeError('no payload'); }); ''");
    let error = receiver
        .deliver(HostArgument::Revive(&revival))
        .expect_err("a throwing reviver");
    assert_eq!(
        (error.name.as_deref(), error.message.as_str()),
        (Some("RangeError"), "no payload")
    );
    assert_eq!(
        receiver.text(
            "(() => { try { setReviver(() => 1); } catch (e) { return `${e.name}: ${e.message}`; } })()"
        ),
        "TypeError: the transfer reviver is already set"
    );
    assert_eq!(receiver.text("String(host.seen)"), "null");
}

#[test]
fn a_transferred_clone_reads_in_a_realm_on_another_thread() {
    let sender = realm();
    sender.text(
        "(() => {
            const canvas = {};
            mark(canvas, true);
            const message = { tag: 'from A', canvas, again: canvas, bytes: new Uint8Array([7, 8]) };
            message.self = message;
            post(message, [canvas], 11);
            return '';
        })()",
    );
    let posted = sender.take_posted();
    let HostValue::Transferred(clone) = posted.into_iter().next().unwrap() else {
        panic!("not a transferred clone");
    };
    let (send, receive) = mpsc::channel();
    let reader = thread::spawn(move || {
        let receiver = realm();
        receiver.text("setReviver((id) => ({ id })); ''");
        let clone: TransferredClone = receive.recv().unwrap();
        let revival = Revival::new(clone.as_bytes(), &[HostArgument::Number(110.0)]).unwrap();
        receiver.deliver(HostArgument::Revive(&revival)).unwrap();
        receiver.text(
            "(() => {
                const m = host.seen;
                return [m.tag, m.canvas.id, m.canvas === m.again, m.bytes.join(','),
                        m.self === m].join(':');
            })()",
        )
    });
    send.send(clone).unwrap();
    assert_eq!(reader.join().unwrap(), "from A:110:true:7,8:true");
}

#[test]
fn transfer_paths_do_not_leak() {
    let tight = || RuntimeOptions {
        memory_limit: Some(2 * 1024 * 1024),
        ..RuntimeOptions::default()
    };
    let sender = realm_with(tight(), 0);
    let kept = Rc::new(RefCell::new(None));
    let sink = Rc::clone(&kept);
    sender
        .realm
        .register_host_module_function_with_transfer("test:sink", "keep", 2, 0, move |arguments| {
            *sink.borrow_mut() = Some(arguments[0].to_value());
            Ok(HostValue::Undefined)
        })
        .unwrap();
    let mut runtime = sender.runtime;
    let evaluation = sender
        .realm
        .evaluate(
            EvalSource::new("import { keep } from 'test:sink'; globalThis.keep = keep;"),
            EvalOptions {
                source_type: SourceType::Module,
                ..EvalOptions::default()
            },
        )
        .unwrap();
    runtime.drain_pending_jobs(&sender.realm).unwrap();
    assert!(
        sender
            .realm
            .settled_promise_result(&evaluation)
            .unwrap()
            .is_some()
    );
    let refused = sender
        .realm
        .evaluate(
            EvalSource::new(
                "let n = 0;
                 for (let i = 0; i < 20000; i++) {
                     const c = { pad: 'x'.repeat(64) };
                     mark(c, true);
                     keep({ c, list: [c, c], text: 'y'.repeat(64) }, [c]);
                     try { keep({ c }); } catch (e) { n++; }
                     try { keep({ c }, [c, c]); } catch (e) { n++; }
                 }
                 n",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    assert_eq!(refused.as_number(), Some(40000.0));
    let Some(HostValue::Transferred(clone)) = kept.borrow_mut().take() else {
        panic!("the last message");
    };

    let receiver = realm_with(tight(), 0);
    receiver.text("setReviver((d) => ({ d, pad: 'z'.repeat(64) })); ''");
    for round in 0..20000 {
        let revival =
            Revival::new(clone.as_bytes(), &[HostArgument::Number(f64::from(round))]).unwrap();
        receiver.deliver(HostArgument::Revive(&revival)).unwrap();
    }
    assert_eq!(
        receiver.text("[host.seen.c.d, host.seen.list[1] === host.seen.c].join(':')"),
        "19999:true"
    );
    drop(receiver);
    drop(sender.realm);
    drop(runtime);
}
