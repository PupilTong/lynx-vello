//! What every devtools test drives a view with: a sink that records what it
//! is sent, and a client that sends requests and waits for answers and
//! events, pumping the view while it waits.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustc_hash::FxHashSet;
use serde_json::{Value, json};

use super::{DevtoolsSession, DevtoolsSink};
use crate::test_support::{TestEngine, TestViewSpec};

const PATIENCE: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(crate) struct Recorder {
    messages: Mutex<Vec<(Value, Option<String>)>>,
    closed: AtomicUsize,
}

impl DevtoolsSink for Recorder {
    fn message(&self, message: String) {
        let value = serde_json::from_str(&message).expect("the backend writes JSON");
        let thread = std::thread::current().name().map(str::to_owned);
        self.messages.lock().unwrap().push((value, thread));
    }

    fn closed(&self) {
        self.closed.fetch_add(1, Ordering::SeqCst);
    }
}

impl Recorder {
    pub(crate) fn count(&self) -> usize {
        self.messages.lock().unwrap().len()
    }

    pub(crate) fn closed(&self) -> usize {
        self.closed.load(Ordering::SeqCst)
    }

    pub(crate) fn wait_for(
        &self,
        what: &str,
        matches: impl Fn(&Value) -> bool,
    ) -> (Value, Option<String>) {
        self.wait_pumping(None, what, matches)
    }

    pub(crate) fn wait_pumping(
        &self,
        engine: Option<&mut TestEngine>,
        what: &str,
        matches: impl Fn(&Value) -> bool,
    ) -> (Value, Option<String>) {
        let (_, value, thread) = self.wait_indexed(engine, what, |_, value| matches(value));
        (value, thread)
    }

    pub(crate) fn answer(&self, id: i64) -> Value {
        self.wait_for(&format!("answer to {id}"), |value| value["id"] == id)
            .0
    }

    fn wait_indexed(
        &self,
        mut engine: Option<&mut TestEngine>,
        what: &str,
        matches: impl Fn(usize, &Value) -> bool,
    ) -> (usize, Value, Option<String>) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some((index, (value, thread))) = self
                .messages
                .lock()
                .unwrap()
                .iter()
                .enumerate()
                .find(|(index, (value, _))| matches(*index, value))
            {
                return (index, value.clone(), thread.clone());
            }
            assert!(Instant::now() < deadline, "no {what} arrived");
            if let Some(engine) = engine.as_deref_mut() {
                let _ = engine.pump();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

pub(crate) fn attach(engine: &TestEngine) -> (Arc<Recorder>, DevtoolsSession) {
    let recorder = Arc::new(Recorder::default());
    let session = engine
        .view
        .devtools_attach(Arc::clone(&recorder) as Arc<dyn DevtoolsSink>)
        .expect("the view takes a frontend");
    (recorder, session)
}

pub(crate) fn booted(spec: TestViewSpec) -> TestEngine {
    spec.with_devtools().boot()
}

pub(crate) struct Client {
    pub(crate) recorder: Arc<Recorder>,
    pub(crate) session: DevtoolsSession,
    next_id: Cell<i64>,
    taken: RefCell<FxHashSet<usize>>,
}

impl Client {
    pub(crate) fn attach(engine: &TestEngine) -> Self {
        let (recorder, session) = attach(engine);
        Self {
            recorder,
            session,
            next_id: Cell::new(1_000_000),
            taken: RefCell::default(),
        }
    }

    pub(crate) fn call(
        &self,
        engine: &mut TestEngine,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, Value> {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let mut message = json!({"id": id, "method": method});
        message["params"] = params;
        if let Some(session_id) = session_id {
            message["sessionId"] = json!(session_id);
        }
        self.session.send(message.to_string());
        let (answer, _) = self.recorder.wait_pumping(
            Some(engine),
            &format!("answer to {method} ({id})"),
            |value| value["id"] == id,
        );
        match answer.get("error") {
            Some(error) => Err(error.clone()),
            None => Ok(answer["result"].clone()),
        }
    }

    pub(crate) fn result(
        &self,
        engine: &mut TestEngine,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> Value {
        self.call(engine, session_id, method, params)
            .unwrap_or_else(|error| panic!("{method} answered an error: {error}"))
    }

    pub(crate) fn error(
        &self,
        engine: &mut TestEngine,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> Value {
        match self.call(engine, session_id, method, params) {
            Ok(result) => panic!("{method} answered a result: {result}"),
            Err(error) => error,
        }
    }

    pub(crate) fn event(
        &self,
        engine: &mut TestEngine,
        session_id: Option<&str>,
        method: &str,
    ) -> Value {
        let (index, event, _) =
            self.recorder
                .wait_indexed(Some(engine), &format!("{method} event"), |index, value| {
                    value["method"] == method
                        && value.get("sessionId").and_then(Value::as_str) == session_id
                        && !self.taken.borrow().contains(&index)
                });
        self.taken.borrow_mut().insert(index);
        event["params"].clone()
    }
}
