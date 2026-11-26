//! Debugger reflection over a realm's values, behind the `devtools` feature.
//!
//! `Context::object_class`, `own_properties`, `own_property`, `prototype` and
//! `describe` run no JavaScript for any input. A `Proxy` (revoked or not, a
//! callable one included) is recognised by its class ID before anything reads
//! it, and those reads answer `Reflected::Proxy` without reading it. The class
//! IDs are read once per runtime, at the first of these calls, from objects
//! made in a new realm on the runtime that no code has run in.
//!
//! - `object_class`: `None` for a primitive. In order: `Proxy`, `Array`, `Error`, `Promise`, then
//!   `TypedArray`, `Map`, `Set`, `Date` and `RegExp` by class ID (so a subclass instance is its
//!   base class, and a prototype's `constructor` changes nothing), then `Function` and `Object`. A
//!   promise's result is read from its internal slot, which neither handles a rejection nor reports
//!   one. A typed array's `length` is its element count, 0 when its buffer is detached or it is out
//!   of bounds.
//! - `own_properties`: every own key, string keys (array indices first, in order) and then symbol
//!   keys, enumerable or not, with its flags and its value or its getter and setter; no getter is
//!   called. A string key with ill-formed UTF-16 is converted lossily; a symbol key carries its
//!   description, empty for a symbol made without one (`QuickJS`'s public API does not tell
//!   `Symbol()` from `Symbol("")`). Fails for a primitive and for a module namespace binding that
//!   is not initialized yet. `OwnKeys::NonIndexed` leaves out the array-index keys without reading
//!   them: the key table lists them first, so it is walked from its end and stops at the first one.
//! - `own_property`: the same for one key named by a string, `None` when the object has no such own
//!   property.
//! - `prototype`: `None` for a null prototype.
//! - `describe`: `JS_PrintValue`'s text, under `DescribeLimits` (0 is no limit; a depth of 0 is
//!   `QuickJS`'s own maximum).
//!
//! `json_stringify` (the intrinsic `JSON.stringify`, not the realm's `JSON`
//! binding; `None` when the value has no JSON text), `json_parse` and
//! `function_with_values` run JavaScript. A value host function's `Fn`
//! handler is handed its realm, through which it can call a function
//! `Value` (`Context::call`), and its arguments as `Value`s; it is
//! re-entrant, so a call it makes may call the same function again. The
//! function holds its realm weakly, so it makes no reference cycle with it,
//! and a call made from another realm of the runtime throws.

use std::ffi::{c_char, c_int, c_void};
use std::ptr::{self, NonNull};
use std::rc::{Rc, Weak};

use smallvec::SmallVec;

use super::{
    Context, ContextInner, Error, ErrorKind, ErrorPhase, HOST_INLINE_ARGS, HostFunctionError,
    HostValue, QJS_PROMISE_FULFILLED, QJS_PROMISE_PENDING, RawValue, Value, ValueKind,
    capture_exception, ffi,
};

#[derive(Debug)]
pub enum ObjectClass {
    Proxy,
    Array,
    Error,
    Promise(PromiseState),
    TypedArray { length: usize },
    Map,
    Set,
    Date,
    RegExp,
    Function,
    Object,
}

#[derive(Debug)]
pub enum PromiseState {
    Pending,
    Fulfilled(Value),
    Rejected(Value),
}

#[derive(Debug)]
pub enum Reflected<T> {
    Proxy,
    Read(T),
}

#[derive(Debug)]
pub enum PropertyKey {
    String(String),
    Symbol { description: String, symbol: Value },
}

#[derive(Debug)]
pub enum PropertySlot {
    Data {
        value: Value,
        writable: bool,
    },
    Accessor {
        getter: Option<Value>,
        setter: Option<Value>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnKeys {
    All,
    NonIndexed,
}

#[derive(Debug)]
pub struct OwnProperty {
    pub key: PropertyKey,
    pub enumerable: bool,
    pub configurable: bool,
    pub slot: PropertySlot,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DescribeLimits {
    pub depth: u32,
    pub string_length: u32,
    pub item_count: u32,
}

type ValueHandler = Box<dyn Fn(&Context, &[Value]) -> Result<HostValue, HostFunctionError>>;

pub(super) struct ValueSlot {
    handler: ValueHandler,
    realm: Weak<ContextInner>,
}

impl ValueSlot {
    pub(super) fn dispatch(
        &self,
        context: *mut ffi::QjsContext,
        count: usize,
        arguments: *const ffi::QjsHostArg,
    ) -> Result<HostValue, HostFunctionError> {
        let raw: &[ffi::QjsHostArg] = if count == 0 {
            &[]
        } else {
            // SAFETY: the value trampoline passes `count` initialized
            // descriptors, valid for this call.
            unsafe { std::slice::from_raw_parts(arguments, count) }
        };
        let boxes = raw.iter().filter_map(|argument| {
            debug_assert_eq!(argument.kind, ffi::HOST_ARG_VALUE);
            NonNull::new(argument.text.cast_mut().cast::<ffi::QjsValue>())
        });
        let realm = self
            .realm
            .upgrade()
            .filter(|realm| realm.raw.as_ptr() == context);
        let Some(realm) = realm else {
            if let Some(context) = NonNull::new(context) {
                boxes.for_each(|raw| drop(RawValue::new(context, raw)));
            }
            return Err(HostFunctionError::new(
                "this host function was called from a realm other than its own",
            ));
        };
        let values: SmallVec<[Value; HOST_INLINE_ARGS]> = boxes
            .map(|raw| Value::from_raw(Rc::clone(&realm), raw))
            .collect();
        (self.handler)(&Context { inner: realm }, &values)
    }
}

unsafe extern "C" fn describe_write(opaque: *mut c_void, bytes: *const c_char, length: usize) {
    if length == 0 {
        return;
    }
    // SAFETY: `opaque` is the `Vec<u8>` `describe` lent for the call, and
    // `bytes` is `length` bytes QuickJS is writing.
    unsafe {
        (*opaque.cast::<Vec<u8>>())
            .extend_from_slice(std::slice::from_raw_parts(bytes.cast(), length));
    }
}

struct AdoptedProperty {
    name: Option<Value>,
    symbol: Option<Value>,
    value: Option<Value>,
    getter: Option<Value>,
    setter: Option<Value>,
    flags: i32,
}

const PHASE: ErrorPhase = ErrorPhase::ConvertValue;

impl Context {
    pub fn object_class(&self, value: &Value) -> Result<Option<ObjectClass>, Error> {
        self.reclaim();
        self.ensure_affinity(value, PHASE)?;
        let raw = value.inner.value.raw.as_ptr();
        let mut length = 0usize;
        // SAFETY: `value` is a live value of this realm; the out-parameter
        // is this frame's.
        let class =
            unsafe { ffi::qjs_devtools_classify(self.raw().as_ptr(), raw, &raw mut length) };
        Ok(Some(match class {
            ffi::CLASS_PRIMITIVE => return Ok(None),
            ffi::CLASS_PROXY => ObjectClass::Proxy,
            ffi::CLASS_ARRAY => ObjectClass::Array,
            ffi::CLASS_ERROR => ObjectClass::Error,
            ffi::CLASS_TYPED_ARRAY => ObjectClass::TypedArray { length },
            ffi::CLASS_MAP => ObjectClass::Map,
            ffi::CLASS_SET => ObjectClass::Set,
            ffi::CLASS_DATE => ObjectClass::Date,
            ffi::CLASS_REGEXP => ObjectClass::RegExp,
            ffi::CLASS_FUNCTION => ObjectClass::Function,
            ffi::CLASS_OBJECT => ObjectClass::Object,
            ffi::CLASS_PROMISE => {
                // SAFETY: as above; the value is a promise.
                let state =
                    unsafe { ffi::qjs_value_promise_state(self.raw().as_ptr(), raw.cast_const()) };
                if state == QJS_PROMISE_PENDING {
                    ObjectClass::Promise(PromiseState::Pending)
                } else {
                    // SAFETY: as above.
                    let result = unsafe {
                        ffi::qjs_value_promise_result(self.raw().as_ptr(), raw.cast_const())
                    };
                    let result = self.value_or_exception(result, PHASE)?;
                    ObjectClass::Promise(if state == QJS_PROMISE_FULFILLED {
                        PromiseState::Fulfilled(result)
                    } else {
                        PromiseState::Rejected(result)
                    })
                }
            }
            _ => return Err(capture_exception(self.raw(), PHASE)),
        }))
    }

    pub fn own_properties(
        &self,
        object: &Value,
        keys: OwnKeys,
    ) -> Result<Reflected<Vec<OwnProperty>>, Error> {
        self.reclaim();
        self.ensure_object(object)?;
        let mut properties = ptr::null_mut();
        let mut count = 0usize;
        // SAFETY: `object` is a live object of this realm; the out-parameters
        // are this frame's.
        let status = unsafe {
            ffi::qjs_devtools_own_properties(
                self.raw().as_ptr(),
                object.inner.value.raw.as_ptr(),
                c_int::from(keys == OwnKeys::NonIndexed),
                &raw mut properties,
                &raw mut count,
            )
        };
        match status {
            ffi::REFLECT_PROXY => return Ok(Reflected::Proxy),
            ffi::REFLECT_READ => {}
            _ => return Err(capture_exception(self.raw(), PHASE)),
        }
        let adopted: Vec<AdoptedProperty> = if count == 0 {
            Vec::new()
        } else {
            // SAFETY: the shim filled `count` entries, whose values this
            // takes ownership of once each.
            unsafe { std::slice::from_raw_parts(properties, count) }
                .iter()
                .map(|property| self.adopt_property(property))
                .collect()
        };
        // SAFETY: the array is the shim's, and nothing reads it after this.
        unsafe { ffi::qjs_devtools_own_properties_free(properties) };
        adopted
            .into_iter()
            .map(Self::own_property_of)
            .collect::<Result<_, _>>()
            .map(Reflected::Read)
    }

    pub fn own_property(
        &self,
        object: &Value,
        name: &str,
    ) -> Result<Reflected<Option<OwnProperty>>, Error> {
        self.reclaim();
        self.ensure_object(object)?;
        let mut property = ffi::QjsOwnProperty {
            name: ptr::null_mut(),
            symbol: ptr::null_mut(),
            value: ptr::null_mut(),
            getter: ptr::null_mut(),
            setter: ptr::null_mut(),
            flags: 0,
        };
        // SAFETY: `object` is a live object of this realm; `name` is
        // `name.len()` bytes; the out-parameter is this frame's.
        let status = unsafe {
            ffi::qjs_devtools_own_property(
                self.raw().as_ptr(),
                object.inner.value.raw.as_ptr(),
                name.as_ptr().cast(),
                name.len(),
                &raw mut property,
            )
        };
        match status {
            ffi::REFLECT_PROXY => Ok(Reflected::Proxy),
            ffi::REFLECT_ABSENT => Ok(Reflected::Read(None)),
            ffi::REFLECT_READ => Self::own_property_of(self.adopt_property(&property))
                .map(|p| Reflected::Read(Some(p))),
            _ => Err(capture_exception(self.raw(), PHASE)),
        }
    }

    pub fn prototype(&self, object: &Value) -> Result<Reflected<Option<Value>>, Error> {
        self.reclaim();
        self.ensure_object(object)?;
        let mut prototype = ptr::null_mut();
        // SAFETY: `object` is a live object of this realm; the out-parameter
        // is this frame's.
        let status = unsafe {
            ffi::qjs_devtools_prototype(
                self.raw().as_ptr(),
                object.inner.value.raw.as_ptr(),
                &raw mut prototype,
            )
        };
        match status {
            ffi::REFLECT_PROXY => Ok(Reflected::Proxy),
            ffi::REFLECT_READ => Ok(Reflected::Read(self.adopt_value(prototype))),
            _ => Err(capture_exception(self.raw(), PHASE)),
        }
    }

    pub fn describe(&self, value: &Value, limits: DescribeLimits) -> Result<String, Error> {
        self.reclaim();
        self.ensure_affinity(value, PHASE)?;
        let mut text: Vec<u8> = Vec::new();
        // SAFETY: `value` is a live value of this realm; `text` outlives the
        // call, which is the only one that writes it.
        unsafe {
            ffi::qjs_devtools_describe(
                self.raw().as_ptr(),
                value.inner.value.raw.as_ptr(),
                limits.depth,
                limits.string_length,
                limits.item_count,
                describe_write,
                (&raw mut text).cast(),
            );
        }
        Ok(String::from_utf8_lossy(&text).into_owned())
    }

    pub fn json_stringify(&self, value: &Value) -> Result<Option<String>, Error> {
        self.reclaim();
        self.ensure_affinity(value, PHASE)?;
        let guard = self.begin();
        // SAFETY: `value` is a live value of this realm.
        let raw = unsafe {
            ffi::qjs_devtools_json_stringify(self.raw().as_ptr(), value.inner.value.raw.as_ptr())
        };
        let result = self.value_or_exception(raw, PHASE).and_then(|text| {
            if text.kind() == ValueKind::Undefined {
                return Ok(None);
            }
            String::from_utf16(&text.to_utf16()?)
                .map(Some)
                .map_err(|_| {
                    Error::bridge(
                        ErrorKind::Engine,
                        PHASE,
                        "QuickJS produced ill-formed JSON text",
                    )
                })
        });
        guard.finish(result, PHASE)
    }

    pub fn json_parse(&self, text: &str) -> Result<Value, Error> {
        self.reclaim();
        let mut terminated = Vec::with_capacity(text.len() + 1);
        terminated.extend_from_slice(text.as_bytes());
        terminated.push(0);
        let guard = self.begin();
        // SAFETY: `terminated` is `text.len()` bytes followed by a NUL.
        let raw = unsafe {
            ffi::qjs_devtools_json_parse(
                self.raw().as_ptr(),
                terminated.as_ptr().cast(),
                text.len(),
            )
        };
        let result = self.value_or_exception(raw, ErrorPhase::ConstructValue);
        guard.finish(result, ErrorPhase::ConstructValue)
    }

    pub fn function_with_values<F>(
        &self,
        name: &str,
        arity: u32,
        handler: F,
    ) -> Result<Value, Error>
    where
        F: Fn(&Self, &[Value]) -> Result<HostValue, HostFunctionError> + 'static,
    {
        self.host_function(
            name,
            arity,
            None,
            super::HostSlot::Values(ValueSlot {
                handler: Box::new(handler),
                realm: Rc::downgrade(&self.inner),
            }),
        )
    }

    fn ensure_object(&self, object: &Value) -> Result<(), Error> {
        self.ensure_affinity(object, PHASE)?;
        if matches!(object.kind(), ValueKind::Object | ValueKind::Function) {
            Ok(())
        } else {
            Err(Error::bridge(
                ErrorKind::TypeMismatch,
                PHASE,
                "value is not an object",
            ))
        }
    }

    fn adopt_value(&self, raw: *mut ffi::QjsValue) -> Option<Value> {
        NonNull::new(raw).map(|raw| Value::from_raw(Rc::clone(&self.inner), raw))
    }

    fn adopt_property(&self, property: &ffi::QjsOwnProperty) -> AdoptedProperty {
        AdoptedProperty {
            name: self.adopt_value(property.name),
            symbol: self.adopt_value(property.symbol),
            value: self.adopt_value(property.value),
            getter: self.adopt_value(property.getter),
            setter: self.adopt_value(property.setter),
            flags: property.flags,
        }
    }

    fn own_property_of(property: AdoptedProperty) -> Result<OwnProperty, Error> {
        let missing = || Error::bridge(ErrorKind::Engine, PHASE, "QuickJS left a property unread");
        let name = String::from_utf16_lossy(
            &property
                .name
                .ok_or_else(missing)?
                .inner
                .value
                .to_utf16_coerced()?,
        );
        let flags = property.flags;
        Ok(OwnProperty {
            key: match property.symbol {
                Some(symbol) => PropertyKey::Symbol {
                    description: name,
                    symbol,
                },
                None => PropertyKey::String(name),
            },
            enumerable: flags & ffi::PROPERTY_ENUMERABLE != 0,
            configurable: flags & ffi::PROPERTY_CONFIGURABLE != 0,
            slot: if flags & ffi::PROPERTY_GETSET != 0 {
                PropertySlot::Accessor {
                    getter: property.getter,
                    setter: property.setter,
                }
            } else {
                PropertySlot::Data {
                    value: property.value.ok_or_else(missing)?,
                    writable: flags & ffi::PROPERTY_WRITABLE != 0,
                }
            },
        })
    }
}
