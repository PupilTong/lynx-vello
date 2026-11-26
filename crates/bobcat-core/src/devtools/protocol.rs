//! The CDP envelope, its error codes, and the `Frontend` every domain
//! answers through.

use std::borrow::Cow;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::Outlet;

pub(crate) struct Request {
    pub(crate) id: i64,
    pub(crate) method: String,
    pub(crate) params: Option<Box<RawValue>>,
}

impl Request {
    pub(crate) fn domain(&self) -> &str {
        self.method.split_once('.').map_or("", |(domain, _)| domain)
    }

    pub(crate) fn params<T: DeserializeOwned>(&self) -> Result<T, Error> {
        serde_json::from_str(self.params.as_deref().map_or("{}", RawValue::get))
            .map_err(|error| Error::invalid_params(error.to_string()))
    }
}

pub(crate) struct Envelope {
    pub(crate) request: Request,
    pub(crate) session_id: Option<String>,
}

pub(crate) fn parse(text: &str) -> Result<Envelope, (Option<i64>, Error)> {
    #[derive(Deserialize)]
    struct Raw<'a> {
        #[serde(borrow, default)]
        id: Option<&'a RawValue>,
        #[serde(borrow, default)]
        method: Option<&'a RawValue>,
        #[serde(default)]
        params: Option<Box<RawValue>>,
        #[serde(rename = "sessionId", default)]
        session_id: Option<String>,
    }
    let raw: Raw<'_> = serde_json::from_str(text).map_err(|error| {
        let error = if error.is_data() {
            Error::invalid_request("Message must be an object")
        } else {
            Error::new(
                PARSE_ERROR,
                format!("Message must be in JSON format: {error}"),
            )
        };
        (None, error)
    })?;
    let id = raw
        .id
        .and_then(|id| serde_json::from_str::<i64>(id.get()).ok())
        .ok_or_else(|| {
            (
                None,
                Error::invalid_request("Message must have integer 'id' property"),
            )
        })?;
    let method = raw
        .method
        .and_then(|method| serde_json::from_str::<String>(method.get()).ok())
        .ok_or_else(|| {
            (
                Some(id),
                Error::invalid_request("Message must have string 'method' property"),
            )
        })?;
    Ok(Envelope {
        request: Request {
            id,
            method,
            params: raw.params,
        },
        session_id: raw.session_id,
    })
}

pub(crate) fn session_id(text: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Routing {
        #[serde(rename = "sessionId", default)]
        session_id: Option<String>,
    }
    serde_json::from_str::<Routing>(text).ok()?.session_id
}

const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const SERVER_ERROR: i32 = -32000;
const SESSION_NOT_FOUND: i32 = -32001;

#[derive(Debug, Serialize)]
pub(crate) struct Error {
    code: i32,
    message: Cow<'static, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
}

impl Error {
    fn new(code: i32, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    fn invalid_request(message: &'static str) -> Self {
        Self::new(INVALID_REQUEST, message)
    }

    pub(crate) fn method_not_found(method: &str) -> Self {
        Self::new(METHOD_NOT_FOUND, format!("'{method}' wasn't found"))
    }

    pub(crate) fn invalid_params(data: impl Into<String>) -> Self {
        Self {
            data: Some(data.into()),
            ..Self::new(INVALID_PARAMS, "Invalid parameters")
        }
    }

    pub(crate) fn server(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(SERVER_ERROR, message)
    }

    pub(crate) fn session_not_found() -> Self {
        Self::new(SESSION_NOT_FOUND, "Session with given id not found.")
    }
}

#[derive(Clone)]
pub(crate) struct Frontend {
    outlet: Arc<Outlet>,
    session_id: Option<Arc<str>>,
}

#[derive(Serialize)]
struct Response<'a, T: ?Sized> {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'a T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a Error>,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>,
}

#[derive(Serialize)]
struct Event<'a, T: ?Sized> {
    method: &'a str,
    params: &'a T,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>,
}

impl Frontend {
    pub(crate) const fn new(outlet: Arc<Outlet>, session_id: Option<Arc<str>>) -> Self {
        Self { outlet, session_id }
    }

    pub(crate) fn is(&self, outlet: &Arc<Outlet>) -> bool {
        Arc::ptr_eq(&self.outlet, outlet)
    }

    pub(crate) const fn outlet(&self) -> &Arc<Outlet> {
        &self.outlet
    }

    pub(crate) fn result<T: Serialize + ?Sized>(&self, id: i64, result: &T) {
        self.write(&Response {
            id: Some(id),
            result: Some(result),
            error: None,
            session_id: self.session_id.as_deref(),
        });
    }

    pub(crate) fn error(&self, id: Option<i64>, error: &Error) {
        self.write(&Response::<()> {
            id,
            result: None,
            error: Some(error),
            session_id: self.session_id.as_deref(),
        });
    }

    pub(crate) fn reply<T: Serialize>(&self, id: i64, reply: Result<T, Error>) {
        match reply {
            Ok(result) => self.result(id, &result),
            Err(error) => self.error(Some(id), &error),
        }
    }

    pub(crate) fn not_found(&self, request: &Request) {
        self.error(Some(request.id), &Error::method_not_found(&request.method));
    }

    pub(crate) fn event<T: Serialize + ?Sized>(&self, method: &str, params: &T) {
        self.write(&Event {
            method,
            params,
            session_id: self.session_id.as_deref(),
        });
    }

    fn write(&self, message: &impl Serialize) {
        if let Ok(text) = serde_json::to_string(message) {
            self.outlet.send(text);
        }
    }
}
